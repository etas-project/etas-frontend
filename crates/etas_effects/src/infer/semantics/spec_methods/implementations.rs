use super::super::{
    engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource,
};
use crate::facts::{
    CallableEnvironment, CallableEnvironmentId, CallableGenericParameter, CallableGenericValue,
};
use etas_package_metadata::{CallableValueScope, CallableValueSource};
use etas_types::{GenericInstantiationFact, SpecMethodFact};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    pub(super) fn capture_external_spec_implementation(
        &self,
        implementation: &etas_types::method::MethodSignatureFact,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        declaration: &etas_types::CallableSignature,
        method: &SpecMethodFact,
        arguments: GenericInstantiationFact,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let etas_types::method::MethodTarget::External { identity } = &implementation.target else {
            return Err(invalid("external method witness does not target metadata"));
        };
        let arguments = method
            .implementation_instantiation(declaration, &implementation.signature, &arguments)
            .map_err(|reason| invalid(&reason))?;
        self.capture_external_method(
            identity,
            &implementation.signature,
            invocation,
            snapshot,
            arguments,
        )
    }

    pub(super) fn capture_external_method(
        &self,
        identity: &etas_code::MethodId,
        signature: &etas_types::CallableSignature,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        arguments: GenericInstantiationFact,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let graph = self
            .implementation_identities
            .get(identity)
            .and_then(|index| *index)
            .and_then(|index| self.external_implementations.get(index))
            .ok_or_else(|| invalid("external method has no unique checked behavior graph"))?;
        let [CallableValueSource::Value(value)] = graph.graph.sources.as_slice() else {
            return Err(invalid(
                "external implementation does not own its callable root",
            ));
        };
        let scope = CallableValueScope::Value(*value);
        let mut generics = std::collections::BTreeMap::new();
        for (position, parameter) in signature.generic_params.iter().enumerate() {
            let value = match parameter.kind {
                etas_types::CallableGenericParamKind::Type => CallableGenericValue::Type(
                    arguments
                        .type_bindings
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, ty)| *ty)
                        .ok_or_else(|| invalid("external method type argument is missing"))?,
                ),
                etas_types::CallableGenericParamKind::Effect => CallableGenericValue::EffectRow(
                    self.row_from_type_ref(
                        arguments
                            .effect_row_bindings
                            .iter()
                            .find(|(name, _)| name == &parameter.name)
                            .map(|(_, row)| row)
                            .ok_or_else(|| invalid("external method effect argument is missing"))?,
                    ),
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
                            scope,
                            position: position as u32,
                        },
                    },
                    origins.clone(),
                );
            }
        }
        let environment = CallableEnvironmentId {
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
                owner: snapshot.owner,
                parameters,
                generics,
            },
        )?;
        let source = crate::MetadataCallableSource {
            analysis: crate::CallableAnalysisKind::CheckedRevision,
            graph: graph.graph_key,
            value: *value,
            environment: Some(environment),
        };
        self.discovered_external_method_sources
            .borrow_mut()
            .insert(source);
        Ok(LatentSource::Metadata(source))
    }
}
