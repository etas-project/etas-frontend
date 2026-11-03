//! Selected-target support for verified residual monitor plans.
//!
//! This gate checks implemented backend semantics only. It does not open
//! services, inspect credentials or grants, start a scope, request approval
//! or execute any code. A supported plan still requires final revision
//! binding and engine installation evidence before it can run.

use std::collections::BTreeMap;
use std::sync::Arc;

use etas_code::residual::target::{
    ResidualBackendSupport, ResidualTargetCapabilities, validate_backend_support,
};
use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_effects::{FrontendRejectionReason, InterpreterSupport};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    EFFECT_OUTPUT, HIR_OUTPUT, RESIDUAL_BACKEND_SUPPORT, TYPE_OUTPUT, VERIFIED_RESIDUAL_PLANS,
    global_with_diagnostics,
};

/// Support results for exactly the verified plans of one effect run, bound
/// to the target capabilities they were checked against.
#[derive(Debug)]
pub struct ResidualBackendSupportFacts {
    target: etas_code::CodeFingerprint,
    supported: BTreeMap<usize, ResidualBackendSupport>,
}

impl ResidualBackendSupportFacts {
    pub fn support(&self, type_fact_index: usize) -> Option<ResidualBackendSupport> {
        self.supported.get(&type_fact_index).copied()
    }

    /// Every verified plan has a support result for `target`, and no result
    /// refers to a plan that is not verified.
    pub fn covers(
        &self,
        plans: &etas_effects::residual::VerifiedResidualPlans,
        target: &ResidualTargetCapabilities,
    ) -> bool {
        self.target == target.fingerprint()
            && self.supported.len() == plans.plans().count()
            && plans.plans().all(|plan| {
                self.supported
                    .get(&plan.type_fact_index())
                    .is_some_and(|support| support.matches(plan.plan(), target))
            })
    }
}

pub struct ValidateResidualBackendSupportPass;

impl Pass<ProjectContext> for ValidateResidualBackendSupportPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateResidualBackendSupportPass", PassKind::Verify)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                VERIFIED_RESIDUAL_PLANS,
            ]))
            .produces(global_with_diagnostics([RESIDUAL_BACKEND_SUPPORT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        context.residual_support = None;
        let Some(plans) = context.residual_plans.clone() else {
            return PassResult::failed("residual support requires verified residual plans");
        };
        let Some(types) = context.types.as_ref() else {
            return PassResult::failed("residual support requires checked type facts");
        };
        let target = &context.input.options.residual_target;
        let mut supported = BTreeMap::new();
        let mut rejected = Vec::new();
        for plan in plans.plans() {
            if let Some(control) = manager.execution_control() {
                if let Err(interruption) = control.checkpoint() {
                    return PassResult::interrupted(interruption);
                }
            }
            let Some(fact) = types
                .facts
                .trace_spec_conformances
                .get(plan.type_fact_index())
                .filter(|fact| fact.item == plan.item())
            else {
                return PassResult::failed(
                    "verified residual plan does not match a checked trace conformance",
                );
            };
            match validate_backend_support(plan.plan(), target) {
                Ok(support) => {
                    supported.insert(plan.type_fact_index(), support);
                }
                Err(error) => rejected.push((plan.item(), fact.span, error)),
            }
        }
        if !rejected.is_empty() {
            let Some(effects) = context.effects.as_mut() else {
                return PassResult::failed("residual support requires effect facts");
            };
            let facts = Arc::make_mut(&mut effects.facts);
            for (item, span, error) in rejected {
                let diagnostic = Diagnostic::effect_check(
                    EffectDiagnosticCode::TraceSpecResidualUnsupported,
                    span,
                    format!(
                        "trace spec requires runtime residual enforcement that the selected target does not support: {error:?}"
                    ),
                );
                effects.diagnostics.push(diagnostic.clone());
                context.diagnostics.push(diagnostic);
                if let Some(summary) = facts.item_effects.get_mut(&item) {
                    summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::MissingRequirement);
                    facts
                        .interpreter_support
                        .items
                        .insert(item, summary.support.clone());
                }
            }
        }
        context.residual_support = Some(Arc::new(ResidualBackendSupportFacts {
            target: target.fingerprint(),
            supported,
        }));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(RESIDUAL_BACKEND_SUPPORT),
        )
    }
}

/// Executable outputs require a support result for every verified plan.
pub(crate) fn validate_residual_publication(context: &ProjectContext) -> Result<(), &'static str> {
    let plans = context
        .residual_plans
        .as_ref()
        .ok_or("executable output requires verified residual plans")?;
    let support = context
        .residual_support
        .as_ref()
        .ok_or("executable output requires residual backend support")?;
    if !support.covers(plans, &context.input.options.residual_target) {
        return Err("residual backend support does not cover the verified plans");
    }
    Ok(())
}
