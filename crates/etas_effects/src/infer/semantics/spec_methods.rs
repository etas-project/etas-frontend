use super::engine::EffectSemantics;
use crate::infer::unit::EffectUnit;

mod dependencies;
mod evidence;
mod implementations;
mod metadata;
mod metadata_graphs;
mod object_metadata;
mod objects;
mod rows;
pub(super) use evidence::MethodEvidence;
pub(crate) use metadata_graphs::MethodCallbackGraph;

fn deferred_completion(
    summary: &mut crate::EffectSummary,
    target: crate::ParameterCallTarget,
    output: etas_types::TypeId,
    span: etas_core::Span,
) {
    // Completion remains conditional on the checked witness, not just its row bound.
    let mut exits = crate::CallableExitSummary::default();
    exits.record(
        crate::CallableOutcome::Returned {
            ty: output,
            observation: crate::ReturnObservation::Any,
        },
        summary.action_trace.clone(),
    );
    exits.gaps.insert(crate::OutcomeAnalysisGap::ParameterCall {
        target,
        output,
        source: span.into(),
    });
    summary.exits = Some(exits);
}

impl EffectSemantics<'_> {
    pub(super) fn checked_method_witness_target(
        &self,
        receiver: etas_types::TypeId,
        args: &[etas_types::TypeId],
        member: &etas_types::SpecMethodIdentity,
    ) -> Option<EffectUnit> {
        match &self
            .types
            .facts
            .methods
            .witness(receiver, args, member)?
            .target
        {
            etas_types::method::MethodTarget::Source { callable, .. }
            | etas_types::method::MethodTarget::Agent { callable, .. } => {
                Some(EffectUnit::Item(*callable))
            }
            _ => None,
        }
    }
}
