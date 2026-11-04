use std::collections::{HashMap, HashSet};

use etas_hir::{HirArg, HirExpr, HirPat, HirRevision, HirStmt, HirStmtId, ResolveResult};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT};
use crate::{HirOptimizationPassReport, HirOutput, ProjectContext};

pub struct LowerResourceRegionPass;

impl Pass<ProjectContext> for LowerResourceRegionPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("LowerResourceRegionPass", PassKind::Transform)
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
            let hoists = find_hoists(&output.hir, legality);
            rewritten = hoists.len();
            context.hir = Some(if hoists.is_empty() {
                output
            } else {
                let mut editor = output.hir.edit();
                apply_hoists(editor.program_mut(), &hoists);
                let hir = editor
                    .finish()
                    .expect("typed-HIR optimization must preserve tree ownership");
                HirOutput { hir }
            });
        }
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "LowerResourceRegion",
                rewritten,
                rewritten,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn find_hoists(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
) -> HashMap<HirStmtId, HirStmtId> {
    let program = revision.program();
    let mut result = HashMap::new();
    for (_, block) in program.blocks.iter() {
        for statement in &block.stmts {
            let HirStmt::For { iter, body, .. } = program.stmts[*statement] else {
                continue;
            };
            if !known_nonempty_iterable(&program.exprs[iter]) {
                continue;
            }
            for body_statement in &program.blocks[body].stmts {
                let HirStmt::Let { pat, value, .. } = program.stmts[*body_statement] else {
                    continue;
                };
                if !matches!(program.pats[pat], HirPat::Binding { .. })
                    || !legality.is_movable(value)
                    || !static_resource_acquisition(program, value)
                {
                    continue;
                }
                result.insert(*statement, *body_statement);
                break;
            }
        }
    }
    result
}

fn known_nonempty_iterable(expression: &HirExpr) -> bool {
    matches!(
        expression,
        HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } if !elems.is_empty()
    )
}

fn static_resource_acquisition(
    program: &etas_hir::HirProgram,
    expression: etas_hir::HirExprId,
) -> bool {
    let HirExpr::Call {
        callee,
        generic_args,
        args,
        ..
    } = &program.exprs[expression]
    else {
        return false;
    };
    let HirExpr::Path(path) = &program.exprs[*callee] else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let name = &program
        .symbols
        .get(symbol)
        .expect("callee symbol must exist")
        .name;
    generic_args.is_empty()
        && (name.starts_with("open_")
            || name.starts_with("acquire_")
            || name.starts_with("lease_")
            || name.starts_with("connect_"))
        && args.iter().all(|argument| match argument {
            HirArg::Positional(value) | HirArg::Named { value, .. } => {
                matches!(program.exprs[*value], HirExpr::Literal(_))
            }
        })
}

fn apply_hoists(program: &mut etas_hir::HirProgram, hoists: &HashMap<HirStmtId, HirStmtId>) {
    let parent_blocks = program
        .blocks
        .iter()
        .map(|(id, block)| (id, block.stmts.clone()))
        .collect::<Vec<_>>();
    let moved = hoists.values().copied().collect::<HashSet<_>>();
    for (_, block) in program.blocks.iter_mut() {
        block.stmts.retain(|statement| !moved.contains(statement));
    }
    for (block_id, statements) in parent_blocks {
        if !statements
            .iter()
            .any(|statement| hoists.contains_key(statement))
        {
            continue;
        }
        let block = program
            .blocks
            .get_mut(block_id)
            .expect("parent block must exist");
        let mut rewritten = Vec::with_capacity(block.stmts.len() + 1);
        for statement in statements {
            if let Some(resource) = hoists.get(&statement) {
                rewritten.push(*resource);
            }
            if !moved.contains(&statement) {
                rewritten.push(statement);
            }
        }
        block.stmts = rewritten;
    }
}
