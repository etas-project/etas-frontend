use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirArg, HirBlockId, HirElseBranch, HirExpr, HirExprId, HirFlowBody, HirItem, HirPatId,
    HirRevision, HirStmt, HirStmtId, HirTreeChild, ResolveResult, SymbolDef,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT, TYPE_OUTPUT};
use crate::{
    BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, HirOutput, ProjectContext,
};

pub struct ShortCircuitAgentThresholdPass;

#[derive(Default)]
struct RemovedTree {
    blocks: HashSet<HirBlockId>,
    statements: HashSet<HirStmtId>,
    patterns: HashSet<HirPatId>,
    expressions: HashSet<HirExprId>,
}

struct Replacement {
    selected: HirBlockId,
    removed: RemovedTree,
}

impl Pass<ProjectContext> for ShortCircuitAgentThresholdPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ShortCircuitAgentThresholdPass", PassKind::Transform)
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
        let replacements = find_replacements(&output.hir, legality, &body.expressions);
        let rewritten = replacements.len();
        let affected_units = replacements
            .keys()
            .filter_map(|expression| output.hir.index().expr_item.get(expression))
            .collect::<HashSet<_>>()
            .len();
        if !replacements.is_empty() {
            super::facts::invalidate_replaced_expressions(
                context,
                replacements.iter().flat_map(|(root, replacement)| {
                    std::iter::once(*root).chain(replacement.removed.expressions.iter().copied())
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
                "ShortCircuitAgentThreshold",
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
    expressions: &[HirExprId],
) -> HashMap<HirExprId, Replacement> {
    let program = revision.program();
    let mut replacements = HashMap::new();
    for expression in expressions {
        let HirExpr::If {
            cond,
            then_block,
            else_branch: Some(HirElseBranch::Block(else_block)),
            ..
        } = program.exprs[*expression]
        else {
            continue;
        };
        let Some(value) = threshold_literal(revision, legality, cond) else {
            continue;
        };
        let (selected, discarded) = if value {
            (then_block, else_block)
        } else {
            (else_block, then_block)
        };
        let Some(mut removed) = removed_subtree(revision, discarded) else {
            continue;
        };
        removed.expressions.extend(expression_tree(revision, cond));
        replacements.insert(*expression, Replacement { selected, removed });
    }
    replacements
}

fn threshold_literal(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    expression: HirExprId,
) -> Option<bool> {
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
    let HirExpr::Literal(etas_hir::HirLiteral::Bool { value, .. }) = revision.exprs[*value] else {
        return None;
    };
    (generic_args.is_empty()
        && legality.is_movable(expression)
        && super::contracts::called_item_has_marker(revision, *callee, "AgentThresholdGuard")
        && guard_returns_argument(revision, *callee))
    .then_some(value)
}

fn guard_returns_argument(revision: &HirRevision, callee: HirExprId) -> bool {
    let HirExpr::Path(path) = &revision.exprs[callee] else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = revision.symbols.get(symbol) else {
        return false;
    };
    let SymbolDef::Item { item } = symbol.def else {
        return false;
    };
    let Some(HirItem::Flow(flow)) = revision.items.get(item) else {
        return false;
    };
    let [parameter] = flow.params.as_slice() else {
        return false;
    };
    match flow.body {
        HirFlowBody::Block(block) => {
            let block = &revision.blocks[block];
            let [statement] = block.stmts.as_slice() else {
                return false;
            };
            let HirStmt::Return {
                value: Some(value), ..
            } = revision.stmts[*statement]
            else {
                return false;
            };
            block.final_expr.is_none() && path_resolves_to(revision, value, *parameter)
        }
        HirFlowBody::Expr { expr, .. } => path_resolves_to(revision, expr, *parameter),
    }
}

fn path_resolves_to(
    revision: &HirRevision,
    expression: HirExprId,
    expected: etas_hir::SymbolId,
) -> bool {
    matches!(
        &revision.exprs[expression],
        HirExpr::Path(path) if path.resolution == ResolveResult::Resolved(expected)
    )
}

fn expression_tree(revision: &HirRevision, root: HirExprId) -> HashSet<HirExprId> {
    let view = revision.view();
    let mut expressions = HashSet::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        if !expressions.insert(expression) {
            continue;
        }
        let expression = view
            .expr(expression)
            .expect("threshold expression must remain indexed");
        pending.extend(expression.children().filter_map(|child| match child {
            HirTreeChild::Expr(child) => Some(child),
            _ => None,
        }));
    }
    expressions
}

fn removed_subtree(revision: &HirRevision, root: HirBlockId) -> Option<RemovedTree> {
    let view = revision.view();
    let mut removed = RemovedTree::default();
    let mut pending = vec![HirTreeChild::Block(root)];
    while let Some(node) = pending.pop() {
        match node {
            HirTreeChild::Block(block) => {
                if !removed.blocks.insert(block) {
                    continue;
                }
                let block = view.block(block)?.data();
                pending.extend(block.stmts.iter().copied().map(HirTreeChild::Stmt));
                pending.extend(block.final_expr.map(HirTreeChild::Expr));
            }
            HirTreeChild::Stmt(statement) => {
                if !removed.statements.insert(statement) {
                    continue;
                }
                pending.extend(view.stmt(statement)?.children());
            }
            HirTreeChild::Expr(expression) => {
                if !removed.expressions.insert(expression) {
                    continue;
                }
                pending.extend(view.expr(expression)?.children());
            }
            HirTreeChild::Pattern(pattern) => {
                if !removed.patterns.insert(pattern) {
                    continue;
                }
                pending.extend(view.pattern(pattern)?.children());
            }
            HirTreeChild::HandlerArm(_) => return None,
        }
    }
    Some(removed)
}

fn apply_replacements(
    program: &mut etas_hir::HirProgram,
    replacements: HashMap<HirExprId, Replacement>,
) {
    for (expression, replacement) in replacements {
        for block in replacement.removed.blocks {
            program.mark_block_optimized_out(block);
        }
        for statement in replacement.removed.statements {
            program.mark_statement_optimized_out(statement);
        }
        for pattern in replacement.removed.patterns {
            program.mark_pattern_optimized_out(pattern);
        }
        for removed in replacement.removed.expressions {
            program.mark_expr_optimized_out(removed);
        }
        *program
            .exprs
            .get_mut(expression)
            .expect("threshold expression must exist") = HirExpr::Block(replacement.selected);
    }
}
