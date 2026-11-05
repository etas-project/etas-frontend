use std::collections::HashMap;

use etas_hir::{
    HirArg, HirExpr, HirExprId, HirPat, HirRevision, HirStmt, HirTreeChild, ResolveResult,
    SymbolDef, SymbolId, unresolved_path_from_segments,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT, TYPE_OUTPUT};
use crate::{
    BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, HirOutput, ProjectContext,
};

pub struct VersionedPreparationCsePass;

struct Replacement {
    symbol: SymbolId,
    optimized_out: Vec<HirExprId>,
}

impl Pass<ProjectContext> for VersionedPreparationCsePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("VersionedPreparationCsePass", PassKind::Transform)
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
        let replacements = find_replacements(&output.hir, legality, &body.blocks);
        let rewritten = replacements.len();
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
                "VersionedPreparationCSE",
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

fn find_replacements(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
    blocks: &[etas_hir::HirBlockId],
) -> HashMap<HirExprId, Replacement> {
    let program = revision.program();
    let mut replacements = HashMap::new();
    for block in blocks {
        let block = &program.blocks[*block];
        let mut available = Vec::<(HirExprId, SymbolId)>::new();
        for statement in &block.stmts {
            let HirStmt::Let { pat, value, .. } = program.stmts[*statement] else {
                continue;
            };
            let HirPat::Binding { symbol, .. } = program.pats[pat] else {
                continue;
            };
            if !worth_reusing(&program.exprs[value])
                || !reusable_expression(revision, value, legality)
            {
                continue;
            }
            if let Some((_, existing)) = available
                .iter()
                .find(|(candidate, _)| equivalent_expression(program, *candidate, value))
            {
                replacements.insert(
                    value,
                    Replacement {
                        symbol: *existing,
                        optimized_out: expression_descendants(revision, value),
                    },
                );
            } else {
                available.push((value, symbol));
            }
        }
    }
    replacements
}

fn expression_descendants(revision: &HirRevision, root: HirExprId) -> Vec<HirExprId> {
    let view = revision.view();
    let mut descendants = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        let expression = view
            .expr(expression)
            .expect("indexed expression must remain available");
        for child in expression.children() {
            if let HirTreeChild::Expr(child) = child {
                descendants.push(child);
                pending.push(child);
            }
        }
    }
    descendants
}

fn worth_reusing(expression: &HirExpr) -> bool {
    matches!(
        expression,
        HirExpr::Call { .. }
            | HirExpr::MethodCall { .. }
            | HirExpr::Array { .. }
            | HirExpr::List { .. }
            | HirExpr::Set { .. }
            | HirExpr::Binary { .. }
    )
}

fn reusable_expression(
    revision: &HirRevision,
    root: HirExprId,
    legality: &crate::AgentEffectLegalityFacts,
) -> bool {
    let view = revision.view();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        if !legality.is_version_stable(expression) {
            return false;
        }
        let expression_view = view
            .expr(expression)
            .expect("indexed expression must remain available");
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
                if let ResolveResult::Resolved(symbol) = path.resolution
                    && let Some(symbol) = revision.symbols.get(symbol)
                    && let SymbolDef::Local { binding, .. } = symbol.def
                    && matches!(revision.stmts[binding], HirStmt::Var { .. })
                {
                    return false;
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

fn equivalent_expression(
    program: &etas_hir::HirProgram,
    left: HirExprId,
    right: HirExprId,
) -> bool {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        match (&program.exprs[left], &program.exprs[right]) {
            (HirExpr::Literal(left), HirExpr::Literal(right)) => {
                if !equivalent_literal(left, right) {
                    return false;
                }
            }
            (HirExpr::Path(left), HirExpr::Path(right)) => {
                if left.resolution != right.resolution
                    || left
                        .segments
                        .iter()
                        .map(|segment| &segment.name)
                        .ne(right.segments.iter().map(|segment| &segment.name))
                {
                    return false;
                }
            }
            (HirExpr::Tuple { elems: left, .. }, HirExpr::Tuple { elems: right, .. })
            | (HirExpr::Array { elems: left, .. }, HirExpr::Array { elems: right, .. })
            | (HirExpr::List { elems: left, .. }, HirExpr::List { elems: right, .. })
            | (HirExpr::Set { elems: left, .. }, HirExpr::Set { elems: right, .. }) => {
                if !push_pairs(&mut pending, left, right) {
                    return false;
                }
            }
            (
                HirExpr::ListCons {
                    head: left_head,
                    tail: left_tail,
                    ..
                },
                HirExpr::ListCons {
                    head: right_head,
                    tail: right_tail,
                    ..
                },
            ) => {
                pending.push((*left_head, *right_head));
                pending.push((*left_tail, *right_tail));
            }
            (HirExpr::EmptySequence { .. }, HirExpr::EmptySequence { .. })
            | (HirExpr::EmptyRecordOrMap { .. }, HirExpr::EmptyRecordOrMap { .. }) => {}
            (
                HirExpr::Range {
                    start: left_start,
                    end: left_end,
                    bounds: left_bounds,
                    ..
                },
                HirExpr::Range {
                    start: right_start,
                    end: right_end,
                    bounds: right_bounds,
                    ..
                },
            ) => {
                if left_bounds != right_bounds {
                    return false;
                }
                pending.push((*left_start, *right_start));
                pending.push((*left_end, *right_end));
            }
            (
                HirExpr::Call {
                    callee: left_callee,
                    generic_args: left_generic,
                    args: left_args,
                    ..
                },
                HirExpr::Call {
                    callee: right_callee,
                    generic_args: right_generic,
                    args: right_args,
                    ..
                },
            ) => {
                if !left_generic.is_empty()
                    || !right_generic.is_empty()
                    || !push_arg_pairs(&mut pending, left_args, right_args)
                {
                    return false;
                }
                pending.push((*left_callee, *right_callee));
            }
            (
                HirExpr::MethodCall {
                    receiver: left_receiver,
                    method: left_method,
                    generic_args: left_generic,
                    args: left_args,
                    ..
                },
                HirExpr::MethodCall {
                    receiver: right_receiver,
                    method: right_method,
                    generic_args: right_generic,
                    args: right_args,
                    ..
                },
            ) => {
                if left_method != right_method
                    || !left_generic.is_empty()
                    || !right_generic.is_empty()
                    || !push_arg_pairs(&mut pending, left_args, right_args)
                {
                    return false;
                }
                pending.push((*left_receiver, *right_receiver));
            }
            (
                HirExpr::Field {
                    base: left_base,
                    field: left_field,
                    ..
                },
                HirExpr::Field {
                    base: right_base,
                    field: right_field,
                    ..
                },
            ) => {
                if left_field != right_field {
                    return false;
                }
                pending.push((*left_base, *right_base));
            }
            (
                HirExpr::Index {
                    base: left_base,
                    index: left_index,
                    ..
                },
                HirExpr::Index {
                    base: right_base,
                    index: right_index,
                    ..
                },
            ) => {
                pending.push((*left_base, *right_base));
                pending.push((*left_index, *right_index));
            }
            (
                HirExpr::Unary {
                    op: left_op,
                    expr: left_expr,
                    ..
                },
                HirExpr::Unary {
                    op: right_op,
                    expr: right_expr,
                    ..
                },
            ) => {
                if left_op != right_op {
                    return false;
                }
                pending.push((*left_expr, *right_expr));
            }
            (
                HirExpr::Binary {
                    op: left_op,
                    lhs: left_lhs,
                    rhs: left_rhs,
                    ..
                },
                HirExpr::Binary {
                    op: right_op,
                    lhs: right_lhs,
                    rhs: right_rhs,
                    ..
                },
            ) => {
                if left_op != right_op {
                    return false;
                }
                pending.push((*left_lhs, *right_lhs));
                pending.push((*left_rhs, *right_rhs));
            }
            _ => return false,
        }
    }
    true
}

fn equivalent_literal(left: &etas_hir::HirLiteral, right: &etas_hir::HirLiteral) -> bool {
    match (left, right) {
        (
            etas_hir::HirLiteral::Bool { value: left, .. },
            etas_hir::HirLiteral::Bool { value: right, .. },
        ) => left == right,
        (
            etas_hir::HirLiteral::Int { text: left, .. },
            etas_hir::HirLiteral::Int { text: right, .. },
        )
        | (
            etas_hir::HirLiteral::Float { text: left, .. },
            etas_hir::HirLiteral::Float { text: right, .. },
        )
        | (
            etas_hir::HirLiteral::String { value: left, .. },
            etas_hir::HirLiteral::String { value: right, .. },
        ) => left == right,
        (
            etas_hir::HirLiteral::Char { value: left, .. },
            etas_hir::HirLiteral::Char { value: right, .. },
        ) => left == right,
        _ => false,
    }
}

fn push_pairs(
    pending: &mut Vec<(HirExprId, HirExprId)>,
    left: &[HirExprId],
    right: &[HirExprId],
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    pending.extend(left.iter().copied().zip(right.iter().copied()));
    true
}

fn push_arg_pairs(
    pending: &mut Vec<(HirExprId, HirExprId)>,
    left: &[HirArg],
    right: &[HirArg],
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    for (left, right) in left.iter().zip(right) {
        match (left, right) {
            (HirArg::Positional(left), HirArg::Positional(right)) => {
                pending.push((*left, *right));
            }
            (
                HirArg::Named {
                    name: left_name,
                    value: left_value,
                    ..
                },
                HirArg::Named {
                    name: right_name,
                    value: right_value,
                    ..
                },
            ) if left_name == right_name => pending.push((*left_value, *right_value)),
            _ => return false,
        }
    }
    true
}

fn apply_replacements(
    program: &mut etas_hir::HirProgram,
    replacements: HashMap<HirExprId, Replacement>,
) {
    for (expression, replacement) in replacements {
        for optimized_out in replacement.optimized_out {
            program.mark_expr_optimized_out(optimized_out);
        }
        let span = program.exprs[expression].span(&program.blocks);
        let name = program
            .symbols
            .get(replacement.symbol)
            .expect("replacement binding symbol must exist")
            .name
            .clone();
        let mut path = unresolved_path_from_segments(&[name.as_str()], span);
        path.resolution = ResolveResult::Resolved(replacement.symbol);
        *program
            .exprs
            .get_mut(expression)
            .expect("replacement expression must exist") = HirExpr::Path(path);
    }
}
