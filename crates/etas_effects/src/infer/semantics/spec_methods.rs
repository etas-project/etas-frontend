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

impl super::engine::EffectSemantics<'_> {
    /// A spec member without an Effect bound gets its escaping row from
    /// the selected implementation when the receiver becomes concrete.
    pub(super) fn spec_method_infers_effects(&self, target: crate::ParameterCallTarget) -> bool {
        use crate::ParameterCallTarget;
        match target {
            ParameterCallTarget::SpecMethod { call, .. } => self
                .types
                .facts
                .checked_method_calls
                .get(&call)
                .is_some_and(|method| method.signature.effects.is_none()),
            ParameterCallTarget::MetadataSpecMethod { invocation, .. } => {
                let definitions = self.method_evidence.borrow();
                let method = &definitions[&invocation].invocation;
                self.types
                    .facts
                    .external_specs
                    .signatures
                    .get(&method.spec)
                    .and_then(|spec| {
                        spec.methods
                            .iter()
                            .find(|member| member.name == method.method)
                    })
                    .and_then(|member| member.signature_in(&self.types.facts))
                    .is_some_and(|signature| signature.effects.is_none())
            }
            _ => false,
        }
    }
}

fn contract_completion(summary: &mut crate::EffectSummary, output: etas_types::TypeId) {
    let mut exits = crate::CallableExitSummary::default();
    exits.record(
        crate::CallableOutcome::Returned {
            ty: output,
            observation: crate::ReturnObservation::Any,
        },
        summary.action_trace.clone(),
    );
    summary.exits = Some(exits);
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
