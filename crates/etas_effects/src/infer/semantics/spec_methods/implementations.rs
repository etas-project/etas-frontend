use super::super::{
    engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource,
};
use crate::facts::{
    CallableEnvironment, CallableEnvironmentId, CallableGenericParameter, CallableGenericValue,
};
use etas_package_metadata::{CallableValueScope, CallableValueSource};
use etas_types::{CheckedSpecRef, GenericInstantiationFact, SpecMethodFact, TypeId, TypeUnifier};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    pub(super) fn external_spec_method(
        &self,
        spec: &CheckedSpecRef,
        receiver: TypeId,
        args: &[TypeId],
        name: &str,
    ) -> Result<
        Option<&etas_types::facts::specs::ExternalSpecImplMethodFact>,
        EffectSpecializationError,
    > {
        let normalize = |spec: &CheckedSpecRef| match spec {
            CheckedSpecRef::Source(symbol) => self
                .types
                .facts
                .external_specs
                .aliases
                .get(symbol)
                .cloned()
                .map_or_else(|| spec.clone(), CheckedSpecRef::External),
            _ => spec.clone(),
        };
        let spec = normalize(spec);
        let same = |left, right| {
            let mut unifier = TypeUnifier::new(&self.types.store);
            unifier.unify(left, right).is_ok() && unifier.substitution().iter().next().is_none()
        };
        let mut methods = self
            .types
            .facts
            .external_specs
            .methods
            .iter()
            .filter(|method| {
                normalize(&method.spec) == spec
                    && method.identity.name() == name
                    && same(method.self_type, receiver)
                    && method.args.len() == args.len()
                    && method
                        .args
                        .iter()
                        .zip(args)
                        .all(|(left, right)| same(*left, *right))
            });
        match (methods.next(), methods.next()) {
            (None, _) => Ok(None),
            (Some(method), None) => Ok(Some(method)),
            _ => Err(invalid(
                "external spec invocation has ambiguous checked implementation evidence",
            )),
        }
    }

    pub(super) fn capture_external_spec_implementation(
        &self,
        implementation: &etas_types::facts::specs::ExternalSpecImplMethodFact,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        declaration: &etas_types::CallableSignature,
        method: &SpecMethodFact,
        arguments: GenericInstantiationFact,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let mut graphs = self
            .external_implementations
            .iter()
            .filter(|graph| graph.identity == implementation.identity);
        let (Some(graph), None) = (graphs.next(), graphs.next()) else {
            return Err(invalid(
                "external method has no unique checked behavior graph",
            ));
        };
        let [CallableValueSource::Value(value)] = graph.graph.sources.as_slice() else {
            return Err(invalid(
                "external implementation does not own its callable root",
            ));
        };
        let signature = &implementation.signature;
        let arguments = method
            .implementation_instantiation(declaration, signature, &arguments)
            .map_err(|reason| EffectSpecializationError::InvalidCallableEnvironment { reason })?;
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
