use std::collections::{BTreeSet, HashMap};

use super::{
    super::{engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource},
    objects::ObjectMethodInput,
};
use crate::{
    MetadataObjectSource, ParameterCallTarget,
    facts::{CallableEnvironment, CallableGenericParameter, CallableGenericValue},
};
use etas_package_metadata::{CallableValueSource, ExistentialWitnessTarget};
use etas_types::{Type, TypeSubstitutionEngine, method::MethodTarget};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

pub(super) enum ObjectResolution {
    Implementation(LatentSource),
    Repack(BTreeSet<LatentSource>),
    Deferred,
}

impl EffectSemantics<'_> {
    pub(super) fn metadata_object_method(
        &self,
        source: MetadataObjectSource,
        input: ObjectMethodInput<'_>,
    ) -> Result<ObjectResolution, EffectSpecializationError> {
        let (graph, _, objects, _) = self
            .callable_graph(source.graph)
            .ok_or_else(|| invalid("metadata object has no checked graph"))?;
        let node = graph
            .objects
            .get(source.object as usize)
            .ok_or_else(|| invalid("metadata object node is absent"))?;
        let checked = objects
            .get(source.object as usize)
            .ok_or_else(|| invalid("metadata object has no typed evidence"))?;
        let packed = match source.environment {
            Some(environment) => self.callable_environment(environment)?,
            None => CallableEnvironment {
                owner: input.snapshot.owner,
                parameters: Default::default(),
                generics: Default::default(),
            },
        };
        let mut names = HashMap::new();
        let mut rows = Vec::new();
        for binding in &node.generic_bindings {
            let key = CallableGenericParameter::Graph {
                graph: source.graph,
                scope: binding.scope,
                position: binding.position,
            };
            match packed.generics.get(&key) {
                Some(CallableGenericValue::Type(ty)) => {
                    names.insert(binding.label.clone(), *ty);
                }
                Some(
                    CallableGenericValue::EffectRow(row)
                    | CallableGenericValue::EffectRowFromCall { row, .. },
                ) => rows.push((binding.label.clone(), row.clone())),
                None => return Err(invalid("metadata object lost a lexical generic binding")),
            }
        }
        let rows = self.type_row_arguments(&rows, Some(packed.owner))?;
        let mut substitution = TypeSubstitutionEngine::existing(&self.types.store, &names)
            .with_effect_arguments(&rows);
        let source_type = substitution.substitute(checked.source_type)?;
        let mut selected = None;
        for witness in &checked.witnesses {
            if witness.member != input.method.identity {
                continue;
            }
            let args = witness
                .bound
                .args
                .iter()
                .map(|arg| substitution.substitute(*arg))
                .collect::<Result<Vec<_>, _>>()?;
            if args == input.spec_args && selected.replace(witness).is_some() {
                return Err(invalid("metadata object member evidence is ambiguous"));
            }
        }
        let witness = selected
            .ok_or_else(|| invalid("metadata object has no evidence for the selected member"))?;
        let payload = self.metadata_object_payload(source, &packed, &node.payload_sources)?;
        let mut actual = input.snapshot.clone();
        actual
            .parameters
            .insert(ParameterCallTarget::Interface(0), payload.clone());
        if matches!(
            self.types.store.get(source_type),
            Some(Type::Existential { .. })
        ) {
            return Ok(ObjectResolution::Repack(payload));
        }
        if matches!(self.types.store.get(source_type), Some(Type::Named(named)) if named.source_parameter.is_some())
        {
            return Ok(ObjectResolution::Deferred);
        }
        let implementation = self
            .types
            .facts
            .methods
            .witness(source_type, input.spec_args, &input.method.identity)
            .ok_or_else(|| {
                invalid("metadata object generic recipe has no checked implementation")
            })?;
        if let ExistentialWitnessTarget::Implementation { method, .. } = &witness.target
            && !matches!(&implementation.target, MethodTarget::External { identity } if identity == method)
        {
            return Err(invalid(
                "metadata object target differs from its checked member witness",
            ));
        }
        let arguments = etas_types::GenericInstantiationFact {
            type_bindings: input.arguments.types.clone(),
            effect_row_bindings: input.arguments.effect_rows.clone(),
            deferred_effect_row_obligations: vec![],
        };
        let source = match &implementation.target {
            MethodTarget::Source { callable, .. } => {
                self.discovered_spec_targets
                    .borrow_mut()
                    .entry(input.call)
                    .or_default()
                    .insert(crate::EffectUnit::Item(*callable));
                self.capture_spec_implementation(
                    *callable,
                    input.invocation,
                    actual,
                    input.declaration,
                    input.method,
                    arguments,
                )?
            }
            MethodTarget::External { .. } => self.capture_external_spec_implementation(
                implementation,
                input.invocation,
                actual,
                input.declaration,
                input.method,
                arguments,
            )?,
            _ => {
                return Err(invalid(
                    "metadata object recipe has no executable checked target",
                ));
            }
        };
        Ok(ObjectResolution::Implementation(source))
    }

    fn metadata_object_payload(
        &self,
        source: MetadataObjectSource,
        environment: &CallableEnvironment,
        sources: &[CallableValueSource],
    ) -> Result<BTreeSet<LatentSource>, EffectSpecializationError> {
        let mut result = BTreeSet::new();
        for item in sources {
            match item {
                CallableValueSource::Parameter(parameter) => result.extend(
                    environment
                        .parameters
                        .get(&ParameterCallTarget::Graph {
                            graph: source.graph,
                            parameter: *parameter,
                        })
                        .ok_or_else(|| {
                            invalid("metadata object payload has no captured parameter")
                        })?,
                ),
                CallableValueSource::Existential(object) => {
                    result.insert(LatentSource::MetadataObject(MetadataObjectSource {
                        object: *object,
                        ..source
                    }));
                }
                CallableValueSource::Value(value) | CallableValueSource::FrozenContract(value) => {
                    result.insert(LatentSource::Metadata(crate::MetadataCallableSource {
                        graph: source.graph,
                        value: *value,
                        environment: source.environment,
                        analysis: if matches!(item, CallableValueSource::FrozenContract(_)) {
                            crate::CallableAnalysisKind::FrozenContract
                        } else {
                            crate::CallableAnalysisKind::CheckedRevision
                        },
                    }));
                }
            }
        }
        Ok(result)
    }
}
