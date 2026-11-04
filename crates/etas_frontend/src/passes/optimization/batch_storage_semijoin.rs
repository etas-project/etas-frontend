use std::collections::HashMap;

use etas_hir::{
    HirArg, HirBinaryOp, HirBlockId, HirExpr, HirExprId, HirLiteral, HirPat, HirPatId, HirStmt,
    HirStmtId, ResolveResult, SymbolDef, SymbolId,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT};
use crate::{HirOptimizationPassReport, HirOutput, ProjectContext};

pub struct BatchStorageSemijoinPass;

struct MembershipLoop {
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
    second: MembershipLoop,
}

impl Pass<ProjectContext> for BatchStorageSemijoinPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BatchStorageSemijoinPass", PassKind::Transform)
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
            let fusions = find_fusions(&output.hir, legality);
            rewritten = fusions.values().map(Vec::len).sum();
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
        }
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "BatchStorageSemijoin",
                rewritten,
                rewritten,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn find_fusions(
    revision: &etas_hir::HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
) -> HashMap<HirBlockId, Vec<Fusion>> {
    let program = revision.program();
    let mut result = HashMap::new();
    for (block_id, block) in program.blocks.iter() {
        let mut fusions = Vec::new();
        let mut index = 0;
        while index + 1 < block.stmts.len() {
            let Some(first) = membership_loop(program, block.stmts[index], legality) else {
                index += 1;
                continue;
            };
            let Some(second) = membership_loop(program, block.stmts[index + 1], legality) else {
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
            result.insert(block_id, fusions);
        }
    }
    result
}

fn membership_loop(
    program: &etas_hir::HirProgram,
    statement: HirStmtId,
    legality: &crate::AgentEffectLegalityFacts,
) -> Option<MembershipLoop> {
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
    let HirStmt::If(if_expression) = program.stmts[*body_statement] else {
        return None;
    };
    let HirExpr::If {
        cond,
        then_block,
        else_branch: None,
        ..
    } = program.exprs[if_expression]
    else {
        return None;
    };
    let HirExpr::Binary {
        op: HirBinaryOp::EqEq,
        lhs,
        rhs,
        ..
    } = program.exprs[cond]
    else {
        return None;
    };
    let (loop_reference, probe) = [(lhs, rhs), (rhs, lhs)]
        .into_iter()
        .find(|(candidate, query)| {
            matches!(&program.exprs[*candidate], HirExpr::Path(path) if path.resolution == ResolveResult::Resolved(loop_symbol))
                && matches!(program.exprs[*query], HirExpr::Literal(_))
        })?;
    if !legality.is_movable(cond) || !legality.is_movable(probe) {
        return None;
    }
    let then_data = &program.blocks[then_block];
    let [assignment] = then_data.stmts.as_slice() else {
        return None;
    };
    if then_data.final_expr.is_some() {
        return None;
    }
    let HirStmt::Assign { target, value, .. } = program.stmts[*assignment] else {
        return None;
    };
    if !matches!(
        program.exprs[value],
        HirExpr::Literal(HirLiteral::Bool { value: true, .. })
    ) || !legality.is_movable(value)
    {
        return None;
    }
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
    Some(MembershipLoop {
        statement,
        pattern: *pat,
        loop_symbol,
        iterable: *iter,
        limits: limits.clone(),
        body: *body,
        body_statement: *body_statement,
        target_symbol,
        loop_reference,
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
                .expect("first semijoin body must exist")
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
                unreachable!("validated semijoin loop reference")
            };
            path.resolution = ResolveResult::Resolved(fusion.first_loop_symbol);
            path.segments[0].name = first_name.clone();
            path.syntax_path.segments[0].text = first_name;
        }
        program
            .blocks
            .get_mut(parent)
            .expect("semijoin parent block must exist")
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
            _ => unreachable!("validated semijoin expression tree"),
        }
    }
    expressions
}
