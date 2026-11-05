use std::collections::HashMap;

use etas_hir::{
    HirArg, HirBlockId, HirExpr, HirExprId, HirLiteral, HirPat, HirPatId, HirStmt, HirStmtId,
    ResolveResult, SymbolDef, SymbolId,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT, TYPE_OUTPUT};
use crate::{
    BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, HirOutput, ProjectContext,
};

pub struct ScheduleBoundedPipelinePass;

struct StageLoop {
    statement: HirStmtId,
    pattern: HirPatId,
    loop_symbol: SymbolId,
    iterable: HirExprId,
    limits: Vec<HirExprId>,
    body: HirBlockId,
    body_statement: HirStmtId,
    target_symbol: SymbolId,
    loop_reference: HirExprId,
}

struct Fusion {
    first_body: HirBlockId,
    first_loop_symbol: SymbolId,
    second: StageLoop,
}

impl Pass<ProjectContext> for ScheduleBoundedPipelinePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ScheduleBoundedPipelinePass", PassKind::Transform)
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
        let fusions = find_fusions(&output.hir, legality, &body.blocks);
        let rewritten = fusions.values().map(Vec::len).sum();
        if !fusions.is_empty() {
            super::facts::invalidate_replaced_expressions(
                context,
                fusions.values().flatten().flat_map(|fusion| {
                    expression_tree(output.hir.program(), fusion.second.iterable)
                        .into_iter()
                        .chain(
                            fusion
                                .second
                                .limits
                                .iter()
                                .flat_map(|limit| expression_tree(output.hir.program(), *limit)),
                        )
                }),
            );
        }
        context.hir = Some(if fusions.is_empty() {
            output
        } else {
            let mut editor = output.hir.edit();
            apply_fusions(editor.program_mut(), fusions);
            let hir = editor
                .finish()
                .expect("typed-HIR optimization must preserve tree ownership");
            HirOutput { hir }
        });
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "ScheduleBoundedPipeline",
                HirOptimizationGroup::O3,
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

fn find_fusions(
    revision: &etas_hir::HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    blocks: &[HirBlockId],
) -> HashMap<HirBlockId, Vec<Fusion>> {
    let program = revision.program();
    let mut result = HashMap::new();
    for block_id in blocks {
        let block = &program.blocks[*block_id];
        let mut fusions = Vec::new();
        let mut index = 0;
        while index + 1 < block.stmts.len() {
            let Some(first) = stage_loop(revision, block.stmts[index], legality) else {
                index += 1;
                continue;
            };
            let Some(second) = stage_loop(revision, block.stmts[index + 1], legality) else {
                index += 1;
                continue;
            };
            if first.target_symbol == second.target_symbol
                || !equivalent_literal_sequence(program, first.iterable, second.iterable)
                || iteration_count(program, &first.limits)
                    != iteration_count(program, &second.limits)
            {
                index += 1;
                continue;
            }
            fusions.push(Fusion {
                first_body: first.body,
                first_loop_symbol: first.loop_symbol,
                second,
            });
            index += 2;
        }
        if !fusions.is_empty() {
            result.insert(*block_id, fusions);
        }
    }
    result
}

fn stage_loop(
    revision: &etas_hir::HirRevision,
    statement: HirStmtId,
    legality: &crate::AgentEffectLegalityFacts,
) -> Option<StageLoop> {
    let program = revision.program();
    let HirStmt::For {
        pat,
        iter,
        limits,
        body,
        ..
    } = &program.stmts[statement]
    else {
        return None;
    };
    let HirPat::Binding {
        symbol: loop_symbol,
        ..
    } = program.pats[*pat]
    else {
        return None;
    };
    let element_count = literal_sequence(program, *iter)?.len();
    if iteration_count(program, limits).is_none_or(|count| count < element_count)
        || !legality.is_movable(*iter)
    {
        return None;
    }
    let body_data = &program.blocks[*body];
    let [body_statement] = body_data.stmts.as_slice() else {
        return None;
    };
    if body_data.final_expr.is_some() {
        return None;
    }
    let HirStmt::Assign { target, value, .. } = program.stmts[*body_statement] else {
        return None;
    };
    let HirExpr::Path(target_path) = &program.exprs[target] else {
        return None;
    };
    let ResolveResult::Resolved(target_symbol) = target_path.resolution else {
        return None;
    };
    let target_data = program.symbols.get(target_symbol)?;
    if !matches!(target_data.def, SymbolDef::Local { binding, .. } if matches!(program.stmts[binding], HirStmt::Var { .. }))
    {
        return None;
    }
    let HirExpr::Call {
        callee,
        generic_args,
        args,
        ..
    } = &program.exprs[value]
    else {
        return None;
    };
    let [
        HirArg::Positional(accumulator),
        HirArg::Positional(loop_reference),
    ] = args.as_slice()
    else {
        return None;
    };
    if !generic_args.is_empty()
        || !super::contracts::called_item_has_marker(revision, *callee, "BoundedPipelineStage")
        || !matches!(&program.exprs[*accumulator], HirExpr::Path(path) if path.resolution == ResolveResult::Resolved(target_symbol))
        || !matches!(&program.exprs[*loop_reference], HirExpr::Path(path) if path.resolution == ResolveResult::Resolved(loop_symbol))
        || !legality.is_movable(value)
        || !legality
            .expression(value)
            .expect("checked stage call must have legality facts")
            .version_dependencies
            .iter()
            .all(|dependency| *dependency == target_symbol)
    {
        return None;
    }
    Some(StageLoop {
        statement,
        pattern: *pat,
        loop_symbol,
        iterable: *iter,
        limits: limits.clone(),
        body: *body,
        body_statement: *body_statement,
        target_symbol,
        loop_reference: *loop_reference,
    })
}

fn literal_sequence(
    program: &etas_hir::HirProgram,
    expression: HirExprId,
) -> Option<Vec<&HirLiteral>> {
    let elements = match &program.exprs[expression] {
        HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems,
        _ => return None,
    };
    elements
        .iter()
        .map(|element| match &program.exprs[*element] {
            HirExpr::Literal(literal) => Some(literal),
            _ => None,
        })
        .collect()
}

fn equivalent_literal_sequence(
    program: &etas_hir::HirProgram,
    left: HirExprId,
    right: HirExprId,
) -> bool {
    let same_kind = matches!(
        (&program.exprs[left], &program.exprs[right]),
        (HirExpr::Array { .. }, HirExpr::Array { .. })
            | (HirExpr::List { .. }, HirExpr::List { .. })
    );
    same_kind
        && literal_sequence(program, left)
            .zip(literal_sequence(program, right))
            .is_some_and(|(left, right)| {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| equivalent_literal(left, right))
            })
}

fn equivalent_literal(left: &HirLiteral, right: &HirLiteral) -> bool {
    match (left, right) {
        (HirLiteral::Bool { value: left, .. }, HirLiteral::Bool { value: right, .. }) => {
            left == right
        }
        (HirLiteral::Int { text: left, .. }, HirLiteral::Int { text: right, .. })
        | (HirLiteral::Float { text: left, .. }, HirLiteral::Float { text: right, .. })
        | (HirLiteral::String { value: left, .. }, HirLiteral::String { value: right, .. }) => {
            left == right
        }
        (HirLiteral::Char { value: left, .. }, HirLiteral::Char { value: right, .. }) => {
            left == right
        }
        _ => false,
    }
}

fn iteration_count(program: &etas_hir::HirProgram, limits: &[HirExprId]) -> Option<usize> {
    let [limit] = limits else {
        return None;
    };
    let HirExpr::Call {
        callee,
        generic_args,
        args,
        ..
    } = &program.exprs[*limit]
    else {
        return None;
    };
    let HirExpr::Path(path) = &program.exprs[*callee] else {
        return None;
    };
    let [HirArg::Positional(count)] = args.as_slice() else {
        return None;
    };
    let HirExpr::Literal(HirLiteral::Int { text, .. }) = &program.exprs[*count] else {
        return None;
    };
    (generic_args.is_empty()
        && path.segments.last().map(|segment| segment.name.as_str()) == Some("Iterations"))
    .then(|| text.parse().expect("checked integer literal"))
}

fn apply_fusions(program: &mut etas_hir::HirProgram, fusions: HashMap<HirBlockId, Vec<Fusion>>) {
    for (parent, fusions) in fusions {
        let removed = fusions
            .iter()
            .map(|fusion| fusion.second.statement)
            .collect::<Vec<_>>();
        for fusion in fusions {
            program.mark_statement_optimized_out(fusion.second.statement);
            program.mark_pattern_optimized_out(fusion.second.pattern);
            program.mark_block_optimized_out(fusion.second.body);
            for expression in expression_tree(program, fusion.second.iterable) {
                program.mark_expr_optimized_out(expression);
            }
            for limit in fusion.second.limits {
                for expression in expression_tree(program, limit) {
                    program.mark_expr_optimized_out(expression);
                }
            }
            program
                .blocks
                .get_mut(fusion.first_body)
                .expect("first pipeline body must exist")
                .stmts
                .push(fusion.second.body_statement);
            let first_name = program
                .symbols
                .get(fusion.first_loop_symbol)
                .expect("first loop symbol must exist")
                .name
                .clone();
            let HirExpr::Path(path) = program
                .exprs
                .get_mut(fusion.second.loop_reference)
                .expect("second loop reference must exist")
            else {
                unreachable!("validated pipeline loop reference")
            };
            path.resolution = ResolveResult::Resolved(fusion.first_loop_symbol);
            path.segments[0].name = first_name.clone();
            path.syntax_path.segments[0].text = first_name;
        }
        program
            .blocks
            .get_mut(parent)
            .expect("pipeline parent block must exist")
            .stmts
            .retain(|statement| !removed.contains(statement));
    }
}

fn expression_tree(program: &etas_hir::HirProgram, root: HirExprId) -> Vec<HirExprId> {
    let mut expressions = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        expressions.push(expression);
        match &program.exprs[expression] {
            HirExpr::Call { callee, args, .. } => {
                pending.push(*callee);
                pending.extend(args.iter().map(|arg| match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
                }));
            }
            HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => {
                pending.extend(elems.iter().copied());
            }
            HirExpr::Literal(_) | HirExpr::Path(_) => {}
            _ => unreachable!("validated pipeline expression tree"),
        }
    }
    expressions
}
