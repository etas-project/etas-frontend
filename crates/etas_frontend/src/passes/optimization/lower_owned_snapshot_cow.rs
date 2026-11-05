use std::collections::HashMap;

use etas_hir::{HirArg, HirExpr, HirExprId, HirRevision, HirTreeChild};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT, TYPE_OUTPUT};
use crate::{
    BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, HirOutput, ProjectContext,
};

pub struct LowerOwnedSnapshotCowPass;

struct Replacement {
    value: HirExpr,
    optimized_out: Vec<HirExprId>,
}

impl Pass<ProjectContext> for LowerOwnedSnapshotCowPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("LowerOwnedSnapshotCowPass", PassKind::Transform)
            .scope(PassScope::Unit(BODY_UNIT_KIND))
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                HIR_OPTIMIZATION_LEGALITY,
            ]))
            .produces(ArtifactSet::from([HIR_OUTPUT, TYPE_OUTPUT, EFFECT_OUTPUT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let body = crate::optimization::current_optimization_body(context, pass_context);
        let legality = context
            .optimization_legality
            .as_ref()
            .expect("legality pass must precede transforms");
        let types = std::sync::Arc::clone(
            &context
                .types
                .as_ref()
                .expect("type facts must precede typed-HIR optimization")
                .facts,
        );
        let output = context.hir.take().expect("HIR output should exist");
        let replacements = find_replacements(&output.hir, legality, &types, &body.expressions);
        let rewritten = replacements.len();
        let affected_units = replacements
            .keys()
            .filter_map(|expression| output.hir.index().expr_item.get(expression))
            .collect::<std::collections::HashSet<_>>()
            .len();
        if !replacements.is_empty() {
            super::facts::invalidate_replaced_expressions(
                context,
                replacements.iter().flat_map(|(root, replacement)| {
                    std::iter::once(*root).chain(replacement.optimized_out.iter().copied())
                }),
            );
        }
        context.hir = Some(if replacements.is_empty() {
            output
        } else {
            let mut editor = output.hir.edit();
            apply_replacements(editor.program_mut(), replacements);
            let hir = editor
                .finish()
                .expect("typed-HIR optimization must preserve tree ownership");
            HirOutput { hir }
        });
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "LowerOwnedSnapshotCOW",
                HirOptimizationGroup::O1,
                rewritten,
                rewritten,
                affected_units,
                rewritten != 0,
            ));
        if rewritten == 0 {
            PassResult::unchanged()
        } else {
            PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
        }
    }
}

fn find_replacements(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    types: &etas_types::TypeFacts,
    expressions: &[HirExprId],
) -> HashMap<HirExprId, Replacement> {
    let program = revision.program();
    let mut replacements = HashMap::new();
    for expression in expressions {
        let data = &program.exprs[*expression];
        let HirExpr::MethodCall {
            receiver,
            method,
            generic_args,
            args,
            ..
        } = data
        else {
            continue;
        };
        let [HirArg::Positional(argument)] = args.as_slice() else {
            continue;
        };
        let Some(fact) = types.std_intrinsic_calls.get(expression) else {
            continue;
        };
        if method != "extend"
            || !generic_args.is_empty()
            || !matches!(
                fact.qualified_path.as_slice(),
                [std, collections, operation]
                    if std == "std"
                        && collections == "collections"
                        && matches!(operation.as_str(), "array_extend" | "list_extend")
            )
            || !empty_sequence(&program.exprs[*argument])
            || !matches!(
                program.exprs[*receiver],
                HirExpr::Path(_) | HirExpr::Literal(_)
            )
            || !legality.is_movable(*expression)
        {
            continue;
        }
        let optimized_out = expression_descendants(revision, *expression);
        replacements.insert(
            *expression,
            Replacement {
                value: program.exprs[*receiver].clone(),
                optimized_out,
            },
        );
    }
    replacements
}

fn empty_sequence(expression: &HirExpr) -> bool {
    matches!(
        expression,
        HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Set { elems, .. } if elems.is_empty()
    ) || matches!(expression, HirExpr::EmptySequence { .. })
}

fn expression_descendants(revision: &HirRevision, root: HirExprId) -> Vec<HirExprId> {
    let view = revision.view();
    let mut descendants = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        let expression = view
            .expr(expression)
            .expect("snapshot expression must remain available");
        for child in expression.children() {
            if let HirTreeChild::Expr(child) = child {
                descendants.push(child);
                pending.push(child);
            }
        }
    }
    descendants
}

fn apply_replacements(
    program: &mut etas_hir::HirProgram,
    replacements: HashMap<HirExprId, Replacement>,
) {
    for (expression, replacement) in replacements {
        for optimized_out in replacement.optimized_out {
            program.mark_expr_optimized_out(optimized_out);
        }
        *program
            .exprs
            .get_mut(expression)
            .expect("owned snapshot expression must exist") = replacement.value;
    }
}
