use etas_effects::{
    ActionTraceDomain, Determinism, EffectSummary, InterpreterSupport, RequestInstantiationSummary,
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{
    EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OPTIMIZATION_REPORT, HIR_OUTPUT,
};
use crate::{AgentEffectLegalityFacts, HirOptimizationPassReport, ProjectContext};

pub struct AgentEffectLegalityPass;

impl Pass<ProjectContext> for AgentEffectLegalityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("AgentEffectLegalityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([HIR_OUTPUT, EFFECT_OUTPUT]))
            .produces(ArtifactSet::from([
                HIR_OPTIMIZATION_LEGALITY,
                HIR_OPTIMIZATION_REPORT,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let mut facts = AgentEffectLegalityFacts::default();
        if context.input.options.optimize_hir {
            let effects = context
                .effects
                .as_ref()
                .expect("effect facts must precede HIR optimization");
            for (expression, summary) in &effects.facts.expr_effects {
                if movable(summary) {
                    facts.movable_expressions.insert(*expression);
                }
            }
        }
        let matched = facts.movable_expression_count();
        context.optimization_legality = Some(facts);
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "AgentEffectLegality",
                matched,
                0,
            ));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

fn movable(summary: &EffectSummary) -> bool {
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
        && matches!(summary.action_trace, ActionTraceDomain::Empty)
        && summary.residual_checks.is_empty()
        && summary.determinism == Determinism::Deterministic
        && matches!(summary.support, InterpreterSupport::LocalOnly)
}
