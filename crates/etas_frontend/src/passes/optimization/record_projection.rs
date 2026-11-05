use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirArg, HirExpr, HirExprId, HirFieldInit, HirFlowBody, HirItem, HirRevision, HirStmt,
    HirTreeChild, ResolveResult, SymbolDef,
};

use crate::{AgentEffectLegalityFacts, HirOutput, ProjectContext};

pub(super) struct ProjectionOutcome {
    pub rewritten: usize,
    pub affected_units: usize,
}

struct Replacement {
    value: HirExpr,
    optimized_out: Vec<HirExprId>,
}

pub(super) fn project_marked_records(
    context: &mut ProjectContext,
    expressions: &[HirExprId],
    marker: &str,
) -> ProjectionOutcome {
    let legality = context
        .optimization_legality
        .as_ref()
        .expect("legality pass must precede transforms");
    let output = context.hir.take().expect("HIR output should exist");
    let replacements = find_replacements(&output.hir, legality, expressions, marker);
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
    ProjectionOutcome {
        rewritten,
        affected_units,
    }
}

fn find_replacements(
    revision: &HirRevision,
    legality: &AgentEffectLegalityFacts,
    expressions: &[HirExprId],
    marker: &str,
) -> HashMap<HirExprId, Replacement> {
    let program = revision.program();
    let mut replacements = HashMap::new();
    for expression in expressions {
        let HirExpr::Field { base, field, .. } = &program.exprs[*expression] else {
            continue;
        };
        let HirExpr::Call {
            callee,
            generic_args,
            args,
            ..
        } = &program.exprs[*base]
        else {
            continue;
        };
        if !generic_args.is_empty()
            || !super::contracts::called_item_has_marker(revision, *callee, marker)
        {
            continue;
        }
        let Some(parameter_index) = projected_parameter(revision, *callee, field) else {
            continue;
        };
        let Some(arguments) = positional_arguments(args) else {
            continue;
        };
        let Some(selected) = arguments.get(parameter_index).copied() else {
            continue;
        };
        let mut optimized_out = vec![*base, *callee, selected];
        for argument in arguments {
            if argument == selected {
                if !legality.is_movable(argument) {
                    optimized_out.clear();
                    break;
                }
                continue;
            }
            let Some(tree) = pure_expression_tree(revision, argument, legality) else {
                optimized_out.clear();
                break;
            };
            optimized_out.extend(tree);
        }
        if optimized_out.is_empty() {
            continue;
        }
        optimized_out.sort_unstable_by_key(|expression| expression.0);
        optimized_out.dedup();
        replacements.insert(
            *expression,
            Replacement {
                value: program.exprs[selected].clone(),
                optimized_out,
            },
        );
    }
    replacements
}

fn positional_arguments(args: &[HirArg]) -> Option<Vec<HirExprId>> {
    args.iter()
        .map(|argument| match argument {
            HirArg::Positional(value) => Some(*value),
            HirArg::Named { .. } => None,
        })
        .collect()
}

fn projected_parameter(
    revision: &HirRevision,
    callee: HirExprId,
    selected_name: &str,
) -> Option<usize> {
    let HirExpr::Path(path) = &revision.exprs[callee] else {
        return None;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let SymbolDef::Item { item } = revision.symbols.get(symbol)?.def else {
        return None;
    };
    let HirItem::Flow(flow) = revision.items.get(item)? else {
        return None;
    };
    let returned = match flow.body {
        HirFlowBody::Block(block) => {
            let block = &revision.blocks[block];
            let [statement] = block.stmts.as_slice() else {
                return None;
            };
            let HirStmt::Return {
                value: Some(value), ..
            } = revision.stmts[*statement]
            else {
                return None;
            };
            if block.final_expr.is_some() {
                return None;
            }
            value
        }
        HirFlowBody::Expr { expr, .. } => expr,
    };
    let HirExpr::Record(record) = &revision.exprs[returned] else {
        return None;
    };
    let value = record.fields.iter().find_map(|field| match field {
        HirFieldInit::Named { name, value, .. } if name == selected_name => Some(*value),
        _ => None,
    })?;
    let HirExpr::Path(path) = &revision.exprs[value] else {
        return None;
    };
    let ResolveResult::Resolved(parameter) = path.resolution else {
        return None;
    };
    flow.params
        .iter()
        .position(|candidate| *candidate == parameter)
}

fn pure_expression_tree(
    revision: &HirRevision,
    root: HirExprId,
    legality: &AgentEffectLegalityFacts,
) -> Option<Vec<HirExprId>> {
    let view = revision.view();
    let mut expressions = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        if !legality.is_movable(expression) {
            return None;
        }
        expressions.push(expression);
        let expression = view.expr(expression)?;
        for child in expression.children() {
            let HirTreeChild::Expr(child) = child else {
                return None;
            };
            pending.push(child);
        }
    }
    Some(expressions)
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
            .expect("marked record projection must exist") = replacement.value;
    }
}
