use etas_effects::{
    ActionTraceDomain, Determinism, EffectSummary, InterpreterSupport, RequestInstantiationSummary,
};
use etas_hir::{HirExpr, HirExprId, HirRevision, HirTreeChild, ResolveResult, SymbolDef};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT};
use crate::{
    AgentEffectLegalityFacts, AgentExpressionLegalityFact, HirOptimizationGroup,
    HirOptimizationPassReport, ProjectContext,
};

pub struct AgentEffectLegalityPass;

impl Pass<ProjectContext> for AgentEffectLegalityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AgentEffectLegalityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([HIR_OUTPUT, EFFECT_OUTPUT]))
            .produces(ArtifactSet::one(HIR_OPTIMIZATION_LEGALITY))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let mut facts = AgentEffectLegalityFacts::default();
        let effects = context
            .effects
            .as_ref()
            .expect("effect facts must precede HIR optimization");
        let hir = &context.hir.as_ref().expect("HIR output should exist").hir;
        for (expression, summary) in &effects.facts.expr_effects {
            let Some(owner) = hir.index().expr_item.get(expression).copied() else {
                continue;
            };
            let (reads, version_dependencies) = dependencies(hir, *expression);
            facts.expressions.insert(
                *expression,
                AgentExpressionLegalityFact {
                    owner,
                    reads,
                    version_dependencies,
                    effect_free: effect_free(summary),
                    deterministic: summary.determinism == Determinism::Deterministic,
                    order_stable: order_stable(summary),
                },
            );
        }
        let matched = facts.movable_expression_count();
        let affected_units = facts
            .expressions
            .values()
            .map(|fact| fact.owner)
            .collect::<std::collections::HashSet<_>>()
            .len();
        context.optimization_legality = Some(facts);
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "AgentEffectLegality",
                HirOptimizationGroup::O1,
                matched,
                0,
                affected_units,
                true,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn effect_free(summary: &EffectSummary) -> bool {
    let requests_are_empty = matches!(
        &summary.handler_requests,
        RequestInstantiationSummary::Available {
            actions,
            parameters,
        } if actions.is_empty() && parameters.is_empty()
    );
    requests_are_empty
        && summary.escaping_effects.is_empty()
        && summary.requested_actions.is_empty()
        && summary.default_actions.is_empty()
        && summary.handled_actions.is_empty()
        && matches!(summary.support, InterpreterSupport::LocalOnly)
}

fn order_stable(summary: &EffectSummary) -> bool {
    matches!(summary.action_trace, ActionTraceDomain::Empty)
        && summary.residual_checks.is_empty()
        && summary.trace_spec_obligations.iter().next().is_none()
}

fn dependencies(
    revision: &HirRevision,
    root: HirExprId,
) -> (
    std::collections::HashSet<etas_hir::SymbolId>,
    std::collections::HashSet<etas_hir::SymbolId>,
) {
    let view = revision.view();
    let mut reads = std::collections::HashSet::new();
    let mut version_dependencies = std::collections::HashSet::new();
    let mut pending = vec![root];
    while let Some(expression) = pending.pop() {
        let expression = view
            .expr(expression)
            .expect("effect fact expression must remain indexed");
        if let HirExpr::Path(path) = expression.data()
            && let ResolveResult::Resolved(symbol) = path.resolution
        {
            reads.insert(symbol);
            if let Some(symbol_data) = revision.symbols.get(symbol)
                && let SymbolDef::Local { binding, .. } = symbol_data.def
                && matches!(revision.stmts[binding], etas_hir::HirStmt::Var { .. })
            {
                version_dependencies.insert(symbol);
            }
        }
        pending.extend(expression.children().filter_map(|child| match child {
            HirTreeChild::Expr(child) => Some(child),
            HirTreeChild::Block(_)
            | HirTreeChild::Stmt(_)
            | HirTreeChild::HandlerArm(_)
            | HirTreeChild::Pattern(_) => None,
        }));
    }
    (reads, version_dependencies)
}
