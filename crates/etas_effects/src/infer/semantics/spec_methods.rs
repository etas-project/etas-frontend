mod evidence;
mod implementations;
mod metadata;
mod metadata_graphs;
mod object_metadata;
mod objects;
mod rows;
pub(super) use evidence::MethodEvidence;
pub(crate) use metadata_graphs::MethodCallbackGraph;

pub(super) struct MethodInstantiation {
    pub generics: etas_types::GenericInstantiationFact,
    pub owner_bindings: std::collections::HashMap<etas_types::TypeId, etas_types::TypeId>,
}

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
