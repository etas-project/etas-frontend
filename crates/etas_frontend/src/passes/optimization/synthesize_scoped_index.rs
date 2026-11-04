use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirArg, HirBinaryOp, HirBlockId, HirExpr, HirExprId, HirLiteral, HirPat, HirPatId, HirRevision,
    HirStmt, HirStmtId, HirTreeChild, ResolveResult, SymbolDef,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT};
use crate::{HirOptimizationPassReport, HirOutput, ProjectContext};

pub struct SynthesizeScopedIndexPass;

#[derive(PartialEq, Eq, Hash)]
enum LiteralKey<'a> {
    Bool(bool),
    Int(&'a str),
    Float(&'a str),
    String(&'a str),
    Char(char),
}

struct Replacement {
    target: HirExprId,
    value: HirExprId,
    span: etas_core::Span,
    blocks: Vec<HirBlockId>,
    statements: Vec<HirStmtId>,
    patterns: Vec<HirPatId>,
    expressions: Vec<HirExprId>,
}

impl Pass<ProjectContext> for SynthesizeScopedIndexPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("SynthesizeScopedIndexPass", PassKind::Transform)
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
                    replacements
                        .values()
                        .flat_map(|replacement| replacement.expressions.iter().copied()),
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
                "SynthesizeScopedIndex",
                rewritten,
                rewritten,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn find_replacements(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
) -> HashMap<HirStmtId, Replacement> {
    let program = revision.program();
    let mut replacements = HashMap::new();
    for (_, block) in program.blocks.iter() {
        for statement in &block.stmts {
            let HirStmt::For {
                pat,
                iter,
                limits,
                body,
                span,
            } = &program.stmts[*statement]
            else {
                continue;
            };
            let HirPat::Binding {
                symbol: loop_symbol,
                ..
            } = program.pats[*pat]
            else {
                continue;
            };
            let Some((index, element_count)) = literal_index(program, *iter) else {
                continue;
            };
            if !iteration_limit_covers(program, limits, element_count)
                || !legality.is_movable(*iter)
            {
                continue;
            }
            let body_data = &program.blocks[*body];
            let [if_statement] = body_data.stmts.as_slice() else {
                continue;
            };
            if body_data.final_expr.is_some() {
                continue;
            }
            let HirStmt::If(if_expression) = program.stmts[*if_statement] else {
                continue;
            };
            let HirExpr::If {
                cond,
                then_block,
                else_branch: None,
                ..
            } = program.exprs[if_expression]
            else {
                continue;
            };
            let Some(query) = static_probe(program, cond, loop_symbol) else {
                continue;
            };
            if !index.contains(&literal_key(&program.exprs[query])) {
                continue;
            }
            let then_data = &program.blocks[then_block];
            let [assign_statement] = then_data.stmts.as_slice() else {
                continue;
            };
            if then_data.final_expr.is_some() {
                continue;
            }
            let HirStmt::Assign { target, value, .. } = program.stmts[*assign_statement] else {
                continue;
            };
            if !matches!(
                program.exprs[value],
                HirExpr::Literal(HirLiteral::Bool { value: true, .. })
            ) {
                continue;
            }
            let HirExpr::Path(target_path) = &program.exprs[target] else {
                continue;
            };
            let ResolveResult::Resolved(target_symbol) = target_path.resolution else {
                continue;
            };
            let Some(target_symbol) = program.symbols.get(target_symbol) else {
                continue;
            };
            if !matches!(target_symbol.def, SymbolDef::Local { binding, .. } if matches!(program.stmts[binding], HirStmt::Var { .. }))
                || !legality.is_movable(value)
            {
                continue;
            }

            let preserved = HashSet::from([target, value]);
            let mut expressions = expression_tree(revision, *iter);
            for limit in limits {
                expressions.extend(expression_tree(revision, *limit));
            }
            expressions.extend(expression_tree(revision, if_expression));
            expressions.retain(|expression| !preserved.contains(expression));
            expressions.sort_unstable_by_key(|expression| expression.0);
            expressions.dedup();

            replacements.insert(
                *statement,
                Replacement {
                    target,
                    value,
                    span: *span,
                    blocks: vec![*body, then_block],
                    statements: vec![*if_statement, *assign_statement],
                    patterns: vec![*pat],
                    expressions,
                },
            );
        }
    }
    replacements
}

fn literal_index<'a>(
    program: &'a etas_hir::HirProgram,
    expression: HirExprId,
) -> Option<(HashSet<LiteralKey<'a>>, usize)> {
    let elements = match &program.exprs[expression] {
        HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems,
        _ => return None,
    };
    let mut index = HashSet::with_capacity(elements.len());
    for element in elements {
        if !matches!(program.exprs[*element], HirExpr::Literal(_)) {
            return None;
        }
        index.insert(literal_key(&program.exprs[*element]));
    }
    Some((index, elements.len()))
}

fn literal_key(expression: &HirExpr) -> LiteralKey<'_> {
    match expression {
        HirExpr::Literal(HirLiteral::Bool { value, .. }) => LiteralKey::Bool(*value),
        HirExpr::Literal(HirLiteral::Int { text, .. }) => LiteralKey::Int(text),
        HirExpr::Literal(HirLiteral::Float { text, .. }) => LiteralKey::Float(text),
        HirExpr::Literal(HirLiteral::String { value, .. }) => LiteralKey::String(value),
        HirExpr::Literal(HirLiteral::Char { value, .. }) => LiteralKey::Char(*value),
        _ => unreachable!("scoped index contains only checked literals"),
    }
}

fn static_probe(
    program: &etas_hir::HirProgram,
    condition: HirExprId,
    loop_symbol: etas_hir::SymbolId,
) -> Option<HirExprId> {
    let HirExpr::Binary {
        op: HirBinaryOp::EqEq,
        lhs,
        rhs,
        ..
    } = program.exprs[condition]
    else {
        return None;
    };
    for (candidate, query) in [(lhs, rhs), (rhs, lhs)] {
        let HirExpr::Path(path) = &program.exprs[candidate] else {
            continue;
        };
        if path.resolution == ResolveResult::Resolved(loop_symbol)
            && matches!(program.exprs[query], HirExpr::Literal(_))
        {
            return Some(query);
        }
    }
    None
}

fn iteration_limit_covers(
    program: &etas_hir::HirProgram,
    limits: &[HirExprId],
    element_count: usize,
) -> bool {
    let [limit] = limits else {
        return false;
    };
    let HirExpr::Call {
        callee,
        generic_args,
        args,
        ..
    } = &program.exprs[*limit]
    else {
        return false;
    };
    let HirExpr::Path(path) = &program.exprs[*callee] else {
        return false;
    };
    let [HirArg::Positional(count)] = args.as_slice() else {
        return false;
    };
    let HirExpr::Literal(HirLiteral::Int { text, .. }) = &program.exprs[*count] else {
        return false;
    };
    generic_args.is_empty()
        && path.segments.last().map(|segment| segment.name.as_str()) == Some("Iterations")
        && text.parse::<usize>().expect("checked integer literal") >= element_count
}

fn expression_tree(revision: &HirRevision, root: HirExprId) -> Vec<HirExprId> {
    let view = revision.view();
    let mut expressions = Vec::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        expressions.push(expression);
        let expression = view
            .expr(expression)
            .expect("indexed expression must remain available");
        for child in expression.children() {
            match child {
                HirTreeChild::Expr(child) => pending.push(child),
                HirTreeChild::Block(block) => {
                    let block = &revision.blocks[block];
                    pending.extend(block.final_expr);
                    for statement in &block.stmts {
                        if let HirStmt::Assign { target, value, .. } = revision.stmts[*statement] {
                            pending.push(target);
                            pending.push(value);
                        }
                    }
                }
                HirTreeChild::Stmt(_) | HirTreeChild::HandlerArm(_) | HirTreeChild::Pattern(_) => {}
            }
        }
    }
    expressions
}

fn apply_replacements(
    program: &mut etas_hir::HirProgram,
    replacements: HashMap<HirStmtId, Replacement>,
) {
    for (statement, replacement) in replacements {
        for block in replacement.blocks {
            program.mark_block_optimized_out(block);
        }
        for statement in replacement.statements {
            program.mark_statement_optimized_out(statement);
        }
        for pattern in replacement.patterns {
            program.mark_pattern_optimized_out(pattern);
        }
        for expression in replacement.expressions {
            program.mark_expr_optimized_out(expression);
        }
        *program
            .stmts
            .get_mut(statement)
            .expect("scoped-index statement must exist") = HirStmt::Assign {
            target: replacement.target,
            value: replacement.value,
            span: replacement.span,
        };
    }
}
