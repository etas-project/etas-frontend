use std::collections::{HashMap, HashSet};

use etas_hir::{
    HirArg, HirExpr, HirExprId, HirLiteral, HirPat, HirPatId, HirRevision, HirStmt, HirStmtId,
    HirTreeChild, ResolveResult,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT};
use crate::{HirOptimizationPassReport, HirOutput, ProjectContext};

pub struct QualityAwareContextPolicyPass;

struct Removal {
    pattern: HirPatId,
    expressions: Vec<HirExprId>,
}

impl Pass<ProjectContext> for QualityAwareContextPolicyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("QualityAwareContextPolicyPass", PassKind::Transform)
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
            let removals = find_removals(&output.hir, legality);
            let budget_rewrites = context
                .input
                .options
                .quality_context_token_budget
                .map(|budget| find_budget_rewrites(&output.hir, &removals, budget))
                .unwrap_or_default();
            rewritten = removals.len() + budget_rewrites.len();
            if !removals.is_empty() {
                super::facts::invalidate_replaced_expressions(
                    context,
                    removals
                        .values()
                        .flat_map(|removal| removal.expressions.iter().copied()),
                );
            }
            context.hir = Some(if rewritten == 0 {
                output
            } else {
                let mut editor = output.hir.edit();
                apply_removals(editor.program_mut(), removals);
                apply_budget_rewrites(editor.program_mut(), budget_rewrites);
                let hir = editor
                    .finish()
                    .expect("typed-HIR optimization must preserve tree ownership");
                HirOutput { hir }
            });
        }
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "QualityAwareContextPolicy",
                rewritten,
                rewritten,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn find_budget_rewrites(
    revision: &HirRevision,
    removals: &HashMap<HirStmtId, Removal>,
    budget: usize,
) -> Vec<(HirExprId, String)> {
    let program = revision.program();
    let mut rewrites = Vec::new();
    for (_, block) in program.blocks.iter() {
        for statement in &block.stmts {
            if removals.contains_key(statement) {
                continue;
            }
            let HirStmt::Let { pat, value, .. } = program.stmts[*statement] else {
                continue;
            };
            let HirPat::Binding { .. } = program.pats[pat] else {
                continue;
            };
            let Some(context_literal) = context_literal(revision, value) else {
                continue;
            };
            let HirExpr::Literal(HirLiteral::String { value: text, .. }) =
                &program.exprs[context_literal]
            else {
                continue;
            };
            if let Some(suffix) = token_suffix(text, budget) {
                rewrites.push((context_literal, suffix));
            }
        }
    }
    rewrites
}

fn token_suffix(text: &str, budget: usize) -> Option<String> {
    if budget == 0 {
        return (!text.is_empty()).then(String::new);
    }
    let mut starts = Vec::new();
    let mut in_token = false;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            in_token = false;
        } else if !in_token {
            starts.push(index);
            in_token = true;
        }
    }
    if starts.len() <= budget {
        return None;
    }
    Some(text[starts[starts.len() - budget]..].to_owned())
}

fn apply_budget_rewrites(program: &mut etas_hir::HirProgram, rewrites: Vec<(HirExprId, String)>) {
    for (expression, suffix) in rewrites {
        let HirExpr::Literal(HirLiteral::String { value, .. }) = program
            .exprs
            .get_mut(expression)
            .expect("context literal must remain available")
        else {
            unreachable!("budget rewrite targets a checked string literal")
        };
        *value = suffix;
    }
}

fn find_removals(
    revision: &HirRevision,
    legality: &crate::AgentEffectLegalityFacts,
) -> HashMap<HirStmtId, Removal> {
    let program = revision.program();
    let used_symbols = program
        .exprs
        .iter()
        .filter(|(expression, _)| !program.is_expr_optimized_out(*expression))
        .filter_map(|(_, expression)| match expression {
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(symbol) => Some(symbol),
                _ => None,
            },
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut removals = HashMap::new();
    for (_, block) in program.blocks.iter() {
        for statement in &block.stmts {
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
    for (_, block) in program.blocks.iter_mut() {
        block
            .stmts
            .retain(|statement| !removals.contains_key(statement));
    }
    for (statement, removal) in removals {
        program.mark_statement_optimized_out(statement);
        program.mark_pattern_optimized_out(removal.pattern);
        for expression in removal.expressions {
            program.mark_expr_optimized_out(expression);
        }
    }
}
