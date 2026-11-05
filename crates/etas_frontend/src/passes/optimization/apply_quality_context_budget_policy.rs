use etas_hir::{HirExpr, HirExprId, HirLiteral, HirPat, HirRevision, HirStmt};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT};
use crate::{HirOutput, HirPolicyTransformReport, ProjectContext};

pub struct ApplyQualityContextBudgetPolicyPass;

impl Pass<ProjectContext> for ApplyQualityContextBudgetPolicyPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ApplyQualityContextBudgetPolicyPass", PassKind::Transform)
            .requires(ArtifactSet::from([HIR_OUTPUT, HIR_OPTIMIZATION_LEGALITY]))
            .produces(ArtifactSet::one(HIR_OUTPUT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let budget = context
            .input
            .options
            .quality_context_token_budget
            .expect("budget policy pass is scheduled only with an explicit budget");
        let output = context.hir.take().expect("HIR output should exist");
        let rewrites = find_budget_rewrites(&output.hir, budget);
        let rewritten = rewrites.len();
        context.hir = Some(if rewrites.is_empty() {
            output
        } else {
            let mut editor = output.hir.edit();
            apply_budget_rewrites(editor.program_mut(), rewrites);
            let hir = editor
                .finish()
                .expect("context policy must preserve HIR tree ownership");
            HirOutput { hir }
        });
        context
            .optimization_report
            .record_policy(HirPolicyTransformReport::new(
                "ApplyQualityContextBudgetPolicy",
                rewritten,
                rewritten,
                rewritten != 0,
            ));
        if rewritten == 0 {
            PassResult::unchanged()
        } else {
            PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
        }
    }
}

fn find_budget_rewrites(revision: &HirRevision, budget: usize) -> Vec<(HirExprId, String)> {
    let program = revision.program();
    let mut rewrites = Vec::new();
    for (_, block) in program.blocks.iter() {
        for statement in &block.stmts {
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
    let [etas_hir::HirArg::Positional(value)] = args.as_slice() else {
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
