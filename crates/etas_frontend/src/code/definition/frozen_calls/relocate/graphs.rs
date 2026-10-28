use super::*;

impl Relocation<'_> {
    pub(in crate::code::definition::frozen_calls) fn graph_exits(
        &self,
        origins: &[crate::package_metadata::CallableGraphOrigin],
        scope: &GenericOwner,
    ) -> Result<Vec<Option<CallableExitSummary>>, ProjectionError> {
        use crate::package_metadata::CallableGraphOrigin as O;
        use etas_effects::facts::{
            CallableReferenceAnalysis as R, CapturedCallableAnalysis as C,
            MetadataCallableAnalysis as M,
        };
        origins
            .iter()
            .map(|origin| {
                let summary = match origin {
                    O::Native {
                        unit,
                        environment: None,
                    } => self.original.effects.unit_effects.get(unit),
                    O::Native {
                        unit,
                        environment: Some(environment),
                    } => match self.original.effects.captured_callables.get(
                        &etas_effects::facts::CapturedCallableId {
                            unit: *unit,
                            environment: *environment,
                        },
                    ) {
                        Some(C::Solved(summary)) => Some(summary.as_ref()),
                        _ => None,
                    },
                    O::Reference(reference) => {
                        match self.original.effects.callable_references.get(reference) {
                            Some(R::Solved { summary, .. }) => Some(summary.as_ref()),
                            _ => None,
                        }
                    }
                    O::Metadata(source) => {
                        match self.original.effects.metadata_callables.get(source) {
                            Some(M::Solved { summary, .. }) => Some(summary.as_ref()),
                            _ => None,
                        }
                    }
                }
                .ok_or_else(|| invalid("frozen callable graph has no checked source analysis"))?;
                let Some(exits) = &summary.exits else {
                    return Ok(None);
                };
                if !exits.gaps.is_empty() {
                    return Ok(None);
                }
                let mut relocated = CallableExitSummary::default();
                for exit in &exits.exits {
                    let outcome = match &exit.outcome {
                        CallableOutcome::Returned { ty, observation } => {
                            CallableOutcome::Returned {
                                ty: self.ty(scope, *ty)?,
                                observation: match observation {
                                    ReturnObservation::Variant(symbol) => {
                                        ReturnObservation::Variant(self.variant(*symbol)?)
                                    }
                                    // Graph return edges carry callable identity; the control
                                    // proof must not retain source expression IDs.
                                    ReturnObservation::CallableSources(_) => ReturnObservation::Any,
                                    other => other.clone(),
                                },
                            }
                        }
                        CallableOutcome::Raised { error } => CallableOutcome::Raised {
                            error: self.ty(scope, *error)?,
                        },
                        CallableOutcome::NonReturn => CallableOutcome::NonReturn,
                    };
                    relocated.exits.push(ExitTraceFact {
                        outcome,
                        trace: self.trace(&exit.trace, scope)?,
                    });
                }
                Ok(Some(relocated))
            })
            .collect()
    }

    pub(in crate::code::definition::frozen_calls) fn graph_values(
        &self,
        values: Vec<ExternalCallableValueMetadata>,
        scope: &GenericOwner,
    ) -> Result<Vec<ExternalCallableValueMetadata>, ProjectionError> {
        values
            .into_iter()
            .map(|mut value| {
                value.params = value
                    .params
                    .into_iter()
                    .map(|ty| self.ty(scope, ty))
                    .collect::<Result<_, _>>()?;
                value.output = self.ty(scope, value.output)?;
                for row in [
                    &mut value.public_effects,
                    &mut value.requested_actions,
                    &mut value.handled_requested_actions,
                    &mut value.default_handler_effects.direct_effects,
                    &mut value.default_handler_effects.default_actions,
                ] {
                    self.graph_row(row, scope)?;
                }
                for handler in &mut value.default_handler_effects.handlers {
                    self.graph_effect(&mut handler.action, scope)?;
                    self.graph_row(&mut handler.produced, scope)?;
                }
                let mut pending = vec![&mut value.action_trace];
                if let Some(requests) = &mut value.handler_requests {
                    for action in &mut requests.actions {
                        self.graph_effect(&mut action.action, scope)?;
                        for (_, ty) in action.type_bindings.iter_mut().flatten() {
                            *ty = self.ty(scope, *ty)?;
                        }
                    }
                    for parameter in &mut requests.parameters {
                        for effect in &mut parameter.excluded {
                            self.graph_effect(effect, scope)?;
                        }
                    }
                }
                while let Some(trace) = pending.pop() {
                    match trace {
                        ExternalActionTraceMetadata::Event { action, .. } => {
                            self.graph_effect(action, scope)?
                        }
                        ExternalActionTraceMetadata::Seq(parts)
                        | ExternalActionTraceMetadata::Choice(parts) => pending.extend(parts),
                        ExternalActionTraceMetadata::Repeat(part) => pending.push(part),
                        ExternalActionTraceMetadata::UnknownOrder(actions)
                        | ExternalActionTraceMetadata::Widened { actions, .. } => {
                            for action in actions {
                                self.graph_effect(action, scope)?;
                            }
                        }
                        ExternalActionTraceMetadata::Empty
                        | ExternalActionTraceMetadata::ParameterCall { .. } => {}
                    }
                }
                Ok(value)
            })
            .collect()
    }

    fn graph_row(
        &self,
        row: &mut ExternalEffectRowMetadata,
        scope: &GenericOwner,
    ) -> Result<(), ProjectionError> {
        for effect in &mut row.effects {
            self.graph_effect(effect, scope)?;
        }
        Ok(())
    }

    fn graph_effect(
        &self,
        effect: &mut ExternalEffectMetadata,
        scope: &GenericOwner,
    ) -> Result<(), ProjectionError> {
        let name = self
            .effect_names
            .get(&effect.path.join("."))
            .ok_or_else(|| {
                invalid("frozen callable graph effect has no canonical target identity")
            })?;
        effect.path = name.split('.').map(str::to_owned).collect();
        for arg in &mut effect.args {
            if let EffectArgRef::Type(ty) = arg {
                *ty = self.ty(scope, *ty)?;
            }
        }
        Ok(())
    }

    pub(in crate::code::definition::frozen_calls) fn graph_roots(
        &self,
        graph: &etas_package_metadata::CallableValueGraph,
        key: CodeFingerprint,
        owner: HirItemId,
    ) -> Result<std::collections::BTreeSet<CallableReturnSource>, ProjectionError> {
        graph
            .sources
            .iter()
            .map(|source| match source {
                etas_package_metadata::CallableValueSource::Value(value) => {
                    Ok(CallableReturnSource::Metadata(MetadataCallableSource {
                        graph: key,
                        value: *value,
                        environment: None,
                    }))
                }
                etas_package_metadata::CallableValueSource::Parameter(parameter) => {
                    if parameter.scope != etas_package_metadata::CallableValueScope::Root {
                        return Err(invalid(
                            "frozen return graph has a non-root parameter source",
                        ));
                    }
                    let symbol = unit_parameters(&self.original.hir, EffectUnit::Item(owner))
                        .get(parameter.position as usize)
                        .ok_or_else(|| {
                            invalid("frozen graph references a missing root parameter")
                        })?;
                    Ok(CallableReturnSource::Parameter(
                        *self.parameters.get(symbol).ok_or_else(|| {
                            invalid("frozen graph root parameter has no candidate identity")
                        })?,
                    ))
                }
            })
            .collect()
    }
}
