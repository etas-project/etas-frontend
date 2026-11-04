use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirArg, HirExpr, HirExprId, HirFlowBody, HirItem, HirRevision, HirStmt, ResolveResult, SymbolId,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT};
use crate::{HirOptimizationPassReport, HirOutput, ProjectContext};

pub struct LowerOwnedSnapshotCowPass;

struct Replacement {
    value: HirExpr,
    optimized_out: [HirExprId; 2],
}

impl Pass<ProjectContext> for LowerOwnedSnapshotCowPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("LowerOwnedSnapshotCowPass", PassKind::Transform)
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
            let output = context.hir.take().expect("HIR output should exist");
            let replacements = find_replacements(&output.hir, legality);
            rewritten = replacements.len();
            if !replacements.is_empty() {
                super::facts::invalidate_replaced_expressions(
                    context,
                    replacements.keys().copied(),
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
                "LowerOwnedSnapshotCOW",
                rewritten,
                rewritten,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn find_replacements(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
) -> HashMap<HirExprId, Replacement> {
    let program = revision.program();
    let snapshots = identity_snapshot_flows(program);
    let mut replacements = HashMap::new();
    for (expression, data) in program.exprs.iter() {
        let HirExpr::Call {
            callee,
            generic_args,
            args,
            ..
        } = data
        else {
            continue;
        };
        let HirExpr::Path(callee_path) = &program.exprs[*callee] else {
            continue;
        };
        let ResolveResult::Resolved(callee_symbol) = callee_path.resolution else {
            continue;
        };
        let [HirArg::Positional(argument)] = args.as_slice() else {
            continue;
        };
        if !generic_args.is_empty()
            || !snapshots.contains(&callee_symbol)
            || !matches!(
                program.exprs[*argument],
                HirExpr::Path(_) | HirExpr::Literal(_)
            )
            || !legality.is_movable(expression)
        {
            continue;
        }
        replacements.insert(
            expression,
            Replacement {
                value: program.exprs[*argument].clone(),
                optimized_out: [*callee, *argument],
            },
        );
    }
    replacements
}

fn identity_snapshot_flows(program: &etas_hir::HirProgram) -> HashSet<SymbolId> {
    let mut result = HashSet::new();
    for (_, item) in program.items.iter() {
        let HirItem::Flow(flow) = item else {
            continue;
        };
        let [parameter] = flow.params.as_slice() else {
            continue;
        };
        let name = &program
            .symbols
            .get(flow.symbol)
            .expect("flow symbol must exist")
            .name;
        if !(name.starts_with("snapshot_")
            || name.starts_with("clone_")
            || name.starts_with("copy_"))
        {
            continue;
        }
        let returned = match flow.body {
            HirFlowBody::Block(block) => {
                let block = &program.blocks[block];
                let [statement] = block.stmts.as_slice() else {
                    continue;
                };
                if block.final_expr.is_some() {
                    continue;
                }
                let HirStmt::Return {
                    value: Some(value), ..
                } = program.stmts[*statement]
                else {
                    continue;
                };
                value
            }
            HirFlowBody::Expr { expr, .. } => expr,
        };
        let HirExpr::Path(path) = &program.exprs[returned] else {
            continue;
        };
        if path.resolution == ResolveResult::Resolved(*parameter) {
            result.insert(flow.symbol);
        }
    }
    result
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
