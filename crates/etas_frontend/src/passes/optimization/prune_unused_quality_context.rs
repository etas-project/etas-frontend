use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirArg, HirBlockId, HirExpr, HirExprId, HirLiteral, HirPat, HirPatId, HirRevision, HirStmt,
    HirStmtId, HirTreeChild, ResolveResult,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT, TYPE_OUTPUT};
use crate::{
    BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, HirOutput, ProjectContext,
};

pub struct PruneUnusedQualityContextPass;

struct Removal {
    pattern: HirPatId,
    expressions: Vec<HirExprId>,
}

impl Pass<ProjectContext> for PruneUnusedQualityContextPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PruneUnusedQualityContextPass", PassKind::Transform)
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
        let output = context.hir.take().expect("HIR output should exist");
        let removals = find_removals(&output.hir, legality, &body.blocks, &body.expressions);
        let rewritten = removals.len();
        if !removals.is_empty() {
            super::facts::invalidate_replaced_expressions(
                context,
                removals
                    .values()
                    .flat_map(|removal| removal.expressions.iter().copied()),
            );
        }
        context.hir = Some(if removals.is_empty() {
            output
        } else {
            let mut editor = output.hir.edit();
            apply_removals(editor.program_mut(), removals);
            let hir = editor
                .finish()
                .expect("typed-HIR optimization must preserve tree ownership");
            HirOutput { hir }
        });
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "PruneUnusedQualityContext",
                HirOptimizationGroup::O1,
                rewritten,
                rewritten,
                usize::from(rewritten != 0),
                rewritten != 0,
            ));
        if rewritten == 0 {
            PassResult::unchanged()
        } else {
            PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
        }
    }
}

fn find_removals(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    blocks: &[HirBlockId],
    expressions: &[HirExprId],
) -> HashMap<HirStmtId, Removal> {
    let program = revision.program();
    let used_symbols = expressions
        .iter()
        .filter(|expression| !program.is_expr_optimized_out(**expression))
        .filter_map(|expression| match &program.exprs[*expression] {
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(symbol) => Some(symbol),
                _ => None,
            },
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut removals = HashMap::new();
    for block in blocks {
        for statement in &program.blocks[*block].stmts {
            let HirStmt::Let { pat, value, .. } = program.stmts[*statement] else {
                continue;
            };
            let HirPat::Binding { symbol, .. } = program.pats[pat] else {
                continue;
            };
            if used_symbols.contains(&symbol) || context_literal(revision, value).is_none() {
                continue;
            }
            let Some(expressions) = pure_expression_tree(revision, value, legality) else {
                continue;
            };
            removals.insert(
                *statement,
                Removal {
                    pattern: pat,
                    expressions,
                },
            );
        }
    }
    removals
}

fn context_literal(revision: &HirRevision, expression: HirExprId) -> Option<HirExprId> {
    let HirExpr::Call {
        callee,
        generic_args,
        args,
        ..
    } = &revision.exprs[expression]
    else {
        return None;
    };
    let [HirArg::Positional(value)] = args.as_slice() else {
        return None;
    };
    (generic_args.is_empty()
        && super::contracts::called_item_has_marker(revision, *callee, "QualityContext")
        && matches!(
            revision.exprs[*value],
            HirExpr::Literal(HirLiteral::String { .. })
        ))
    .then_some(*value)
}

fn pure_expression_tree(
    revision: &HirRevision,
    root: HirExprId,
    legality: &crate::AgentEffectLegalityFacts,
) -> Option<Vec<HirExprId>> {
    let view = revision.view();
    let mut expressions = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        if !legality.is_movable(expression) {
            return None;
        }
        expressions.push(expression);
        let expression = view
            .expr(expression)
            .expect("indexed expression must remain available");
        for child in expression.children() {
            match child {
                HirTreeChild::Expr(child) => pending.push(child),
                HirTreeChild::Block(_)
                | HirTreeChild::Stmt(_)
                | HirTreeChild::HandlerArm(_)
                | HirTreeChild::Pattern(_) => return None,
            }
        }
    }
    Some(expressions)
}

fn apply_removals(program: &mut etas_hir::HirProgram, removals: HashMap<HirStmtId, Removal>) {
    let removed_statements = removals.keys().copied().collect::<HashSet<_>>();
    for (_, block) in program.blocks.iter_mut() {
        block
            .stmts
            .retain(|statement| !removed_statements.contains(statement));
    }
    for (statement, removal) in removals {
        program.mark_statement_optimized_out(statement);
        program.mark_pattern_optimized_out(removal.pattern);
        for expression in removal.expressions {
            program.mark_expr_optimized_out(expression);
        }
    }
}
