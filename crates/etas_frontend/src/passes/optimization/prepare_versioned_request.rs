use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirExpr, HirExprId, HirPat, HirPatId, HirRevision, HirStmt, HirStmtId, HirTreeChild,
    ResolveResult, SymbolDef,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT};
use crate::{
    BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, HirOutput, ProjectContext,
};

pub struct PrepareVersionedRequestPass;

impl Pass<ProjectContext> for PrepareVersionedRequestPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PrepareVersionedRequestPass", PassKind::Transform)
            .scope(PassScope::Unit(BODY_UNIT_KIND))
            .requires(ArtifactSet::from([HIR_OUTPUT, HIR_OPTIMIZATION_LEGALITY]))
            .produces(ArtifactSet::one(HIR_OUTPUT))
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
        let hoists = find_hoists(&output.hir, legality, &body.blocks);
        let rewritten = hoists.values().map(Vec::len).sum();
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
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "PrepareVersionedRequest",
                HirOptimizationGroup::O2,
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

fn find_hoists(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    blocks: &[etas_hir::HirBlockId],
) -> HashMap<HirStmtId, Vec<HirStmtId>> {
    let program = revision.program();
    let mut result = HashMap::new();
    for block in blocks {
        for statement in &program.blocks[*block].stmts {
            let HirStmt::For {
                pat, iter, body, ..
            } = &program.stmts[*statement]
            else {
                continue;
            };
            if !known_nonempty_iterable(program.exprs.get(*iter)) {
                continue;
            }
            let mut loop_symbols = HashSet::new();
            collect_pattern_symbols(program, *pat, &mut loop_symbols);
            for body_statement in &program.blocks[*body].stmts {
                match &program.stmts[*body_statement] {
                    HirStmt::Let { pat, .. } | HirStmt::Var { pat, .. } => {
                        collect_pattern_symbols(program, *pat, &mut loop_symbols);
                    }
                    _ => {}
                }
            }
            let mut hoisted_symbols = HashSet::new();
            let mut candidates = Vec::new();
            for body_statement in &program.blocks[*body].stmts {
                let HirStmt::Let { pat, value, .. } = &program.stmts[*body_statement] else {
                    break;
                };
                let HirPat::Binding { symbol, .. } = program.pats[*pat] else {
                    break;
                };
                if !legality.is_movable(*value)
                    || !invariant_expression(
                        revision,
                        *value,
                        legality,
                        &loop_symbols,
                        &hoisted_symbols,
                    )
                    || reusable_resource_acquisition(revision, *value)
                {
                    break;
                }
                candidates.push(*body_statement);
                hoisted_symbols.insert(symbol);
            }
            if !candidates.is_empty() {
                result.insert(*statement, candidates);
            }
        }
    }
    result
}

fn reusable_resource_acquisition(revision: &HirRevision, expression: HirExprId) -> bool {
    let HirExpr::Call { callee, .. } = revision.exprs[expression] else {
        return false;
    };
    super::contracts::called_item_has_marker(revision, callee, "ReusableResourceFactory")
}

fn known_nonempty_iterable(expression: Option<&HirExpr>) -> bool {
    matches!(
        expression,
        Some(HirExpr::Array { elems, .. })
            | Some(HirExpr::List { elems, .. })
            | Some(HirExpr::Tuple { elems, .. })
            | Some(HirExpr::Set { elems, .. }) if !elems.is_empty()
    )
}

fn invariant_expression(
    revision: &HirRevision,
    root: HirExprId,
    legality: &crate::AgentEffectLegalityFacts,
    loop_symbols: &HashSet<etas_hir::SymbolId>,
    hoisted_symbols: &HashSet<etas_hir::SymbolId>,
) -> bool {
    let view = revision.view();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        if !legality.is_version_stable(expression) {
            return false;
        }
        let Some(expression_view) = view.expr(expression) else {
            return false;
        };
        match expression_view.data() {
            HirExpr::Perform { .. }
            | HirExpr::Infer { .. }
            | HirExpr::Handler { .. }
            | HirExpr::Handle { .. }
            | HirExpr::If { .. }
            | HirExpr::Match { .. }
            | HirExpr::Lambda { .. }
            | HirExpr::Block(_)
            | HirExpr::Try { .. }
            | HirExpr::Error { .. } => return false,
            HirExpr::Path(path) => {
                if let ResolveResult::Resolved(symbol) = path.resolution {
                    if loop_symbols.contains(&symbol) && !hoisted_symbols.contains(&symbol) {
                        return false;
                    }
                    if let Some(symbol) = revision.symbols.get(symbol)
                        && let SymbolDef::Local { binding, .. } = symbol.def
                        && matches!(revision.stmts[binding], HirStmt::Var { .. })
                    {
                        return false;
                    }
                }
            }
            _ => {}
        }
        for child in expression_view.children() {
            match child {
                HirTreeChild::Expr(child) => pending.push(child),
                HirTreeChild::Block(_)
                | HirTreeChild::Stmt(_)
                | HirTreeChild::HandlerArm(_)
                | HirTreeChild::Pattern(_) => return false,
            }
        }
    }
    true
}

fn collect_pattern_symbols(
    program: &etas_hir::HirProgram,
    root: HirPatId,
    symbols: &mut HashSet<etas_hir::SymbolId>,
) {
    let mut pending = vec![root];
    while let Some(pattern) = pending.pop() {
        match &program.pats[pattern] {
            HirPat::Binding { symbol, .. } => {
                symbols.insert(*symbol);
            }
            HirPat::Tuple { elems, .. } => pending.extend(elems.iter().copied()),
            HirPat::Record { fields, .. } => {
                pending.extend(fields.iter().filter_map(|field| field.pat));
            }
            HirPat::Variant { args, .. } => pending.extend(args.iter().copied()),
            HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => {}
        }
    }
}

fn apply_hoists(program: &mut etas_hir::HirProgram, hoists: &HashMap<HirStmtId, Vec<HirStmtId>>) {
    let parent_blocks = program
        .blocks
        .iter()
        .map(|(id, block)| (id, block.stmts.clone()))
        .collect::<Vec<_>>();
    let moved = hoists.values().flatten().copied().collect::<HashSet<_>>();
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
        let mut rewritten = Vec::with_capacity(block.stmts.len() + moved.len());
        for statement in statements {
            if let Some(candidates) = hoists.get(&statement) {
                rewritten.extend(candidates.iter().copied());
            }
            if !moved.contains(&statement) {
                rewritten.push(statement);
            }
        }
        block.stmts = rewritten;
    }
}
