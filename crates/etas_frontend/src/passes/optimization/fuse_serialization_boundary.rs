use std::collections::HashMap;

use etas_hir::{HirArg, HirExpr, HirExprId, HirRevision, HirTreeChild};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT};
use crate::{HirOptimizationPassReport, HirOutput, ProjectContext};

pub struct FuseSerializationBoundaryPass;

struct Replacement {
    value: HirExpr,
    optimized_out: Vec<HirExprId>,
}

impl Pass<ProjectContext> for FuseSerializationBoundaryPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("FuseSerializationBoundaryPass", PassKind::Transform)
            .requires(ArtifactSet::from([HIR_OUTPUT, HIR_OPTIMIZATION_LEGALITY]))
            .produces(ArtifactSet::from([HIR_OUTPUT, HIR_OPTIMIZATION_REPORT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let mut rewritten = 0;
        if context.input.options.optimize_hir {
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
            let replacements = find_replacements(&output.hir, legality, &types);
            rewritten = replacements.len();
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
        }
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "FuseSerializationBoundary",
                rewritten,
                rewritten,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn find_replacements(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    types: &etas_types::TypeFacts,
) -> HashMap<HirExprId, Replacement> {
    let program = revision.program();
    let mut replacements = HashMap::new();
    for (expression, _) in program.exprs.iter() {
        let Some(parse_call) = unary_method_receiver(program, expression, "unwrap") else {
            continue;
        };
        if !is_intrinsic(types, expression, etas_std::intrinsic::pure::RESULT_UNWRAP)
            || !call_targets(program, parse_call, &["std", "json", "parse"])
        {
            continue;
        }
        let Some(encoded) = unary_call_argument(program, parse_call) else {
            continue;
        };
        let Some(stringify_call) = unary_method_receiver(program, encoded, "unwrap") else {
            continue;
        };
        if !is_intrinsic(types, encoded, etas_std::intrinsic::pure::RESULT_UNWRAP)
            || !call_targets(program, stringify_call, &["std", "json", "stringify"])
        {
            continue;
        }
        let Some(value) = unary_call_argument(program, stringify_call) else {
            continue;
        };
        if !matches!(program.exprs[value], HirExpr::Path(_)) || !legality.is_movable(expression) {
            continue;
        }
        let optimized_out = expression_descendants(revision, expression);
        replacements.insert(
            expression,
            Replacement {
                value: program.exprs[value].clone(),
                optimized_out,
            },
        );
    }
    replacements
}

fn unary_method_receiver(
    program: &etas_hir::HirProgram,
    expression: HirExprId,
    expected_method: &str,
) -> Option<HirExprId> {
    let HirExpr::MethodCall {
        receiver,
        method,
        generic_args,
        args,
        ..
    } = &program.exprs[expression]
    else {
        return None;
    };
    (method == expected_method && generic_args.is_empty() && args.is_empty()).then_some(*receiver)
}

fn unary_call_argument(program: &etas_hir::HirProgram, expression: HirExprId) -> Option<HirExprId> {
    let HirExpr::Call {
        generic_args, args, ..
    } = &program.exprs[expression]
    else {
        return None;
    };
    let [HirArg::Positional(value)] = args.as_slice() else {
        return None;
    };
    generic_args.is_empty().then_some(*value)
}

fn is_intrinsic(types: &etas_types::TypeFacts, expression: HirExprId, intrinsic: u32) -> bool {
    types
        .std_intrinsic_calls
        .get(&expression)
        .is_some_and(|fact| fact.intrinsic == intrinsic)
}

fn call_targets(program: &etas_hir::HirProgram, expression: HirExprId, expected: &[&str]) -> bool {
    let HirExpr::Call { callee, .. } = program.exprs[expression] else {
        return false;
    };
    let HirExpr::Path(path) = &program.exprs[callee] else {
        return false;
    };
    let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    matches!(
        &program
            .symbols
            .get(symbol)
            .expect("resolved JSON intrinsic symbol must exist")
            .def,
        etas_hir::SymbolDef::ImportAlias { path, .. }
            if path.iter().map(String::as_str).eq(expected.iter().copied())
    )
}

fn expression_descendants(revision: &HirRevision, root: HirExprId) -> Vec<HirExprId> {
    let view = revision.view();
    let mut descendants = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        let expression = view
            .expr(expression)
            .expect("serialization expression must remain available");
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
            .expect("serialization boundary expression must exist") = replacement.value;
    }
}
