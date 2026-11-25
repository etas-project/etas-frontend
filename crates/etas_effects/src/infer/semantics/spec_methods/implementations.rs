use super::super::{
    engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource,
};
use crate::facts::{
    CallableEnvironment, CallableEnvironmentId, CallableGenericParameter, CallableGenericValue,
};
use etas_package_metadata::{CallableValueScope, CallableValueSource};
use etas_types::SpecMethodFact;

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    pub(super) fn external_method_graph(
        &self,
        identity: &etas_code::MethodId,
    ) -> Result<(&crate::ExternalSpecImplementationMetadata, u32), EffectSpecializationError> {
        let implementation = self
            .implementation_identities
            .get(identity)
            .and_then(|index| *index)
            .and_then(|index| self.external_implementations.get(index))
            .ok_or_else(|| {
                invalid(&format!(
                    "external method {identity:?} has no unique checked behavior graph"
                ))
            })?;
        let [CallableValueSource::Value(root)] = implementation.graph.sources.as_slice() else {
            return Err(invalid(
                "external implementation does not own its callable root",
            ));
        };
        Ok((implementation, *root))
    }

    pub(super) fn capture_external_spec_implementation(
        &self,
        implementation: &etas_types::method::MethodSignatureFact,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        declaration: &etas_types::CallableSignature,
        method: &SpecMethodFact,
        arguments: super::MethodInstantiation,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let etas_types::method::MethodTarget::External { identity } = &implementation.target else {
            return Err(invalid("external method witness does not target metadata"));
        };
        let owner_bindings = arguments.owner_bindings;
        let arguments = method
            .implementation_instantiation(
                declaration,
                &implementation.signature,
                &arguments.generics,
            )
            .map_err(|reason| invalid(&reason))?;
        self.capture_external_method(
            identity,
            &implementation.signature,
            invocation,
            snapshot,
            &owner_bindings,
            (
                arguments.type_bindings,
                arguments
                    .effect_row_bindings
                    .iter()
                    .map(|(name, row)| (name.clone(), self.row_from_type_ref(row)))
                    .collect(),
            ),
        )
    }

    pub(in crate::infer::semantics) fn capture_external_method(
        &self,
        identity: &etas_code::MethodId,
        signature: &etas_types::CallableSignature,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        owner_bindings: &std::collections::HashMap<etas_types::TypeId, etas_types::TypeId>,
        arguments: super::super::callable_generics::GenericSubstitutions,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let (graph, value) = self.external_method_graph(identity)?;
        let scope = CallableValueScope::Value(value);
        let mut generics = std::collections::BTreeMap::new();
        for (position, parameter) in graph.owner_params.iter().enumerate() {
            let argument = owner_bindings
                .get(&parameter.subject)
                .ok_or_else(|| invalid("external method owner argument is missing"))?;
            generics.insert(
                CallableGenericParameter::Graph {
                    graph: graph.graph_key,
                    scope: CallableValueScope::Root,
                    position: position as u32,
                },
                CallableGenericValue::Type(*argument),
            );
        }
        for (position, parameter) in signature.generic_params.iter().enumerate() {
            let value = match parameter.kind {
                etas_types::CallableGenericParamKind::Type => CallableGenericValue::Type(
                    arguments
                        .0
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, ty)| *ty)
                        .ok_or_else(|| invalid("external method type argument is missing"))?,
                ),
                etas_types::CallableGenericParamKind::Effect => CallableGenericValue::EffectRow(
                    arguments
                        .1
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, row)| row.clone())
                        .ok_or_else(|| invalid("external method effect argument is missing"))?,
                ),
            };
            generics.insert(
                CallableGenericParameter::Graph {
                    graph: graph.graph_key,
                    scope,
                    position: position as u32,
                },
                value,
            );
        }
        let mut parameters = std::collections::BTreeMap::new();
        for position in 0..signature.params.len() {
            if let Some(origins) = snapshot
                .parameters
                .get(&crate::ParameterCallTarget::Interface(position as u32))
            {
                parameters.insert(
                    crate::ParameterCallTarget::Graph {
                        graph: graph.graph_key,
                        parameter: etas_package_metadata::CallableValueParameter {
                            projection: None,
                            scope,
                            position: position as u32,
                        },
                    },
                    origins.clone(),
                );
            }
        }
        for parameter in graph.graph.parameter_references() {
            if parameter.scope != scope || parameter.projection.is_none() {
                continue;
            }
            let path = graph
                .graph
                .projection_path(parameter)
                .map_err(|error| invalid(&error.to_string()))?;
            let target = self
                .intern_external_parameter_projection(
                    crate::ParameterCallTarget::Interface(parameter.position),
                    path.to_vec(),
                )
                .ok_or_else(|| {
                    invalid("external method argument projection exceeds its checked graph budget")
                })?;
            if let Some(origins) = snapshot.parameters.get(&target) {
                parameters.insert(
                    crate::ParameterCallTarget::Graph {
                        graph: graph.graph_key,
                        parameter,
                    },
                    origins.clone(),
                );
            }
        }
        for (target, origins) in &snapshot.parameters {
            let Some((crate::ParameterCallTarget::Interface(position), path)) =
                self.parameter_projection_target(*target)
            else {
                continue;
            };
            let target = self
                .intern_external_parameter_projection(
                    crate::ParameterCallTarget::Graph {
                        graph: graph.graph_key,
                        parameter: etas_package_metadata::CallableValueParameter {
                            scope,
                            position,
                            projection: None,
                        },
                    },
                    path,
                )
                .ok_or_else(|| invalid("external method capture has no checked projection"))?;
            parameters.insert(target, origins.clone());
        }
        let environment = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&generics),
            instance: None,
            scope: None,
            origin: etas_code::CodeFingerprint::of_parts(
                "etas.effects.external-method-application.v1",
                [
                    graph.graph_key.as_bytes().as_slice(),
                    invocation.origin.as_bytes().as_slice(),
                ],
            ),
            application: invocation.application,
        };
        self.record_callable_environment(
            environment,
            CallableEnvironment {
                invocation_origin: None,
                invocation_inputs: Default::default(),
                owner: snapshot.owner,
                parameters,
                generics,
            },
        )?;
        let call = self
            .capture_site_expression(invocation.application)
            .ok_or_else(|| {
                invalid("external method application has no checked expression identity")
            })?;
        let state = super::super::state::EffectState {
            owner: Some(snapshot.owner),
            ..Default::default()
        };
        let environment = self
            .bind_metadata_environment(
                super::super::metadata_callable::MetadataCallBindings {
                    key: Some(graph.graph_key),
                    scope,
                    environment: Some(environment),
                    arguments: &[],
                    generics: &[],
                    type_bindings: &[],
                    effect_row_bindings: &[],
                },
                &state,
                call,
            )?
            .2;
        let source = crate::MetadataCallableSource {
            analysis: crate::CallableAnalysisKind::CheckedRevision,
            graph: graph.graph_key,
            value,
            environment,
        };
        self.discovered_external_method_sources
            .borrow_mut()
            .insert(source);
        Ok(LatentSource::Metadata(source))
    }
}
