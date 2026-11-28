use crate::infer::analysis::dependencies::InputFact;
use std::collections::{BTreeMap, HashMap};

use etas_code::CodeFingerprint;
use etas_core::Span;
use etas_types::{CallableGenericParamKind, GenericInstantiationFact};

use crate::{
    EffectSummary, ParameterCallTarget,
    facts::{
        CallableEnvironment, CallableEnvironmentId, CallableGenericParameter, CallableGenericValue,
        MetadataSpecMethodDefinition,
    },
    infer::semantics::{
        engine::EffectSemantics,
        specialize::{EffectSpecializationError, TraceArgumentBindings},
        state::EffectState,
    },
};

fn invalid(reason: impl Into<String>) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

use super::super::callable_generics::ScopedGenericArguments as GenericArguments;

impl EffectSemantics<'_> {
    pub(crate) fn register_metadata_method(
        &self,
        invocation: &std::sync::Arc<crate::ExternalSpecMethodInvocation>,
        parameters: &[(String, ParameterCallTarget)],
        generics: &[(String, CallableGenericParamKind)],
        span: Span,
    ) -> Result<ParameterCallTarget, EffectSpecializationError> {
        self.dependencies.read(InputFact::MethodEvidence);
        let declaration = self
            .types
            .facts
            .external_specs
            .signatures
            .get(&invocation.spec)
            .filter(|spec| spec.kind == etas_types::SpecKind::TypeSpec)
            .ok_or_else(|| invalid("metadata method has no checked behavioral spec declaration"))?;
        let method = declaration
            .methods
            .iter()
            .find(|method| method.name == invocation.method)
            .ok_or_else(|| invalid("metadata method has no checked member declaration"))?;
        let signature = method
            .signature_in(&self.types.facts)
            .ok_or_else(|| invalid("metadata member has no checked signature"))?;
        if declaration.param_names.len() != invocation.spec_args.len()
            || signature.params.len() != invocation.arity as usize
            || signature
                .generic_params
                .len()
                .checked_sub(usize::from(method.receiver_param.is_some()))
                != Some(invocation.generic_arguments.len())
        {
            return Err(invalid(
                "metadata method arguments do not match its checked declaration",
            ));
        }
        let definition = MetadataSpecMethodDefinition {
            invocation: invocation.clone(),
            generics: generics.to_vec(),
            parameters: parameters.to_vec(),
            span,
        };
        let key = definition.identity().map_err(invalid)?;
        let mut definitions = self.method_evidence.borrow_mut();
        if let Some(existing) = definitions.get(&key) {
            if &existing.invocation != invocation
                || existing.generics != generics
                || existing.parameters != parameters
            {
                return Err(invalid("metadata method evidence identity is ambiguous"));
            }
        } else {
            if definitions.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid(
                    "metadata method evidence exceeds its resource budget",
                ));
            }
            std::sync::Arc::make_mut(&mut definitions).insert(key, std::sync::Arc::new(definition));
        }
        Ok(ParameterCallTarget::MetadataSpecMethod {
            invocation: key,
            environment: None,
        })
    }

    pub(in crate::infer::semantics) fn bind_metadata_method_environment(
        &self,
        invocation: CodeFingerprint,
        environment: Option<CallableEnvironmentId>,
        application: CodeFingerprint,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        generics: GenericArguments<'_>,
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        self.dependencies.read(InputFact::MethodEvidence);
        let types = generics.types;
        let rows = generics.rows;
        if let Some(environment) = environment {
            return self.instantiate_callable_environment_with_values(
                environment,
                application,
                bindings,
                state,
                generics,
            );
        }
        let definition = self
            .method_evidence
            .borrow()
            .get(&invocation)
            .cloned()
            .ok_or_else(|| invalid("metadata method has no checked evidence definition"))?;
        let owner = state
            .owner
            .ok_or_else(|| invalid("metadata method has no invocation owner"))?;
        let mut snapshot = CallableEnvironment {
            invocation_origin: None,
            invocation_inputs: Default::default(),
            owner,
            parameters: BTreeMap::new(),
            generics: BTreeMap::new(),
        };
        if let Some(parameters) = generics.parameters {
            snapshot
                .generics
                .extend(parameters.iter().map(|(subject, argument)| {
                    (
                        CallableGenericParameter::TypeParameter { subject: *subject },
                        CallableGenericValue::Type(*argument),
                    )
                }));
        }
        for (position, (name, kind)) in definition.generics.iter().enumerate() {
            let value = match kind {
                CallableGenericParamKind::Type => CallableGenericValue::Type(
                    *types
                        .iter()
                        .find(|(parameter, _)| parameter == name)
                        .map(|(_, ty)| ty)
                        .ok_or_else(|| {
                            invalid(format!(
                                "metadata method {}.{} type binding `{name}` is missing; available types {types:?}; interface {:?}",
                                definition.invocation.spec.path.join("."),
                                definition.invocation.method,
                                definition.parameters,
                            ))
                        })?,
                ),
                CallableGenericParamKind::Effect => CallableGenericValue::EffectRow(
                    rows.iter()
                        .find(|(parameter, _)| parameter == name)
                        .map(|(_, row)| row)
                        .cloned()
                        .ok_or_else(|| {
                            invalid(format!("metadata method row binding `{name}` is missing"))
                        })?,
                ),
            };
            snapshot.generics.insert(
                CallableGenericParameter::Graph {
                    graph: invocation,
                    scope: etas_package_metadata::CallableValueScope::Root,
                    position: position as u32,
                },
                value,
            );
        }
        for argument in &definition.invocation.callable_arguments {
            let origins = self.metadata_method_argument_origins(
                argument,
                &definition,
                bindings,
                state,
                (&snapshot, application),
            )?;
            snapshot
                .parameters
                .insert(ParameterCallTarget::Interface(argument.position), origins);
        }
        let id = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&snapshot.generics),
            instance: None,
            scope: Some(self.callable_environment_scope(snapshot.owner)?),
            origin: invocation,
            application,
        };
        self.record_callable_environment(id, snapshot)?;
        Ok(id)
    }

    fn metadata_method_argument_origins(
        &self,
        argument: &crate::ExternalSpecMethodCallableArgument,
        definition: &MetadataSpecMethodDefinition,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        context: (&CallableEnvironment, CodeFingerprint),
    ) -> Result<
        std::collections::BTreeSet<super::super::state::LatentSource>,
        EffectSpecializationError,
    > {
        use super::super::state::LatentSource;
        use etas_package_metadata::{
            CallableValueParameter, CallableValueScope, CallableValueSource,
        };
        let (snapshot, application) = context;
        let graph = super::metadata_graphs::graph_key(
            definition.invocation.identity,
            argument.position,
            &definition.generics,
        );
        let mut captured = CallableEnvironment {
            invocation_origin: None,
            invocation_inputs: Default::default(),
            owner: snapshot.owner,
            generics: BTreeMap::new(),
            parameters: BTreeMap::new(),
        };
        for (parameter, value) in &snapshot.generics {
            if let CallableGenericParameter::TypeParameter { .. } = parameter {
                captured.generics.insert(*parameter, value.clone());
                continue;
            }
            let CallableGenericParameter::Graph {
                scope, position, ..
            } = parameter
            else {
                return Err(invalid(
                    "metadata method callback has an invalid generic owner",
                ));
            };
            captured.generics.insert(
                CallableGenericParameter::Graph {
                    graph,
                    scope: *scope,
                    position: *position,
                },
                value.clone(),
            );
        }
        for (binding_position, (_, parameter)) in definition.parameters.iter().enumerate() {
            let position = match parameter {
                ParameterCallTarget::Interface(position) => *position,
                // Node evidence includes both captured and own parameters.
                // Its callback graph indexes this ordered binding interface.
                ParameterCallTarget::Graph { .. } => binding_position as u32,
                ParameterCallTarget::Native(symbol) => {
                    match self.hir.symbols.get(*symbol).map(|symbol| &symbol.def) {
                        Some(etas_hir::SymbolDef::Param { param_index, .. }) => *param_index,
                        _ => {
                            return Err(invalid(
                                "metadata method callback has no checked native parameter identity",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(invalid(
                        "metadata method callback parameter has no interface identity",
                    ));
                }
            };
            if let Some(binding) = bindings.get(parameter) {
                let origins = self.trace_argument_origins(binding, state)?;
                captured.parameters.insert(
                    ParameterCallTarget::Graph {
                        graph,
                        parameter: CallableValueParameter {
                            projection: None,
                            scope: CallableValueScope::Root,
                            position,
                        },
                    },
                    origins.clone(),
                );
                if let ParameterCallTarget::Graph {
                    graph: parent_graph,
                    parameter: projected,
                } = parameter
                    && projected.projection.is_some()
                {
                    let mut root = *projected;
                    root.projection = None;
                    let root = ParameterCallTarget::Graph {
                        graph: *parent_graph,
                        parameter: root,
                    };
                    let root_position = definition
                        .parameters
                        .iter()
                        .position(|(_, target)| *target == root)
                        .ok_or_else(|| {
                            invalid(
                                "metadata callback projection has no root in its binding interface",
                            )
                        })?;
                    let (parent, _, _, _) =
                        self.callable_graph(*parent_graph).ok_or_else(|| {
                            invalid("metadata callback projection has no checked parent graph")
                        })?;
                    let path = parent
                        .projection_path(*projected)
                        .map_err(|error| invalid(error.to_string()))?;
                    for (projection, selected) in
                        argument.sources.parameter_projections.iter().enumerate()
                    {
                        if selected == path {
                            captured.parameters.insert(
                                ParameterCallTarget::Graph {
                                    graph,
                                    parameter: CallableValueParameter {
                                        scope: CallableValueScope::Root,
                                        position: root_position as u32,
                                        projection: Some(projection as u32),
                                    },
                                },
                                origins.clone(),
                            );
                        }
                    }
                }
            }
        }
        let id = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&captured.generics),
            instance: None,
            scope: Some(self.callable_environment_scope(captured.owner)?),
            origin: graph,
            application,
        };
        self.record_callable_environment(id, captured.clone())?;
        let mut origins = std::collections::BTreeSet::new();
        for source in &argument.sources.sources {
            match source {
                CallableValueSource::Existential(object) => {
                    argument
                        .sources
                        .objects
                        .get(*object as usize)
                        .ok_or_else(|| {
                            invalid("metadata object argument has no checked value node")
                        })?;
                    origins.insert(LatentSource::MetadataObject(crate::MetadataObjectSource {
                        graph,
                        object: *object,
                        environment: Some(id),
                    }));
                }
                CallableValueSource::Parameter(parameter) => {
                    let values = captured
                        .parameters
                        .get(&ParameterCallTarget::Graph {
                            graph,
                            parameter: *parameter,
                        })
                        .cloned();
                    let values = match values {
                        Some(values) => Some(values),
                        None => {
                            let call = definition.parameters.get(parameter.position as usize)
                                .and_then(|(_, target)| bindings.get(target))
                                .and_then(|binding| match binding {
                                    super::super::specialize::TraceArgumentBinding::Expression(expr)
                                    | super::super::specialize::TraceArgumentBinding::ProjectedExpression { expr, .. } => Some(*expr),
                                    super::super::specialize::TraceArgumentBinding::InputData { call, .. } => Some(*call),
                                    _ => None,
                                });
                            match call {
                                Some(call) => self.metadata_parameter_origins(graph, *parameter, &captured, Some(id), state, call)?,
                                None => None,
                            }
                        }
                    }
                        .ok_or_else(|| {
                            invalid(format!(
                                "metadata method callback {}.{} has no checked argument binding for {parameter:?}; interface {:?}; bound inputs {:?}",
                                definition.invocation.spec.path.join("."),
                                definition.invocation.method,
                                definition.parameters,
                                bindings.keys().collect::<Vec<_>>(),
                            ))
                        })?;
                    origins.extend(values);
                }
                CallableValueSource::Value(value) | CallableValueSource::FrozenContract(value) => {
                    origins.insert(LatentSource::Metadata(crate::MetadataCallableSource {
                        analysis: if matches!(source, CallableValueSource::FrozenContract(_)) {
                            crate::CallableAnalysisKind::FrozenContract
                        } else {
                            crate::CallableAnalysisKind::CheckedRevision
                        },
                        graph,
                        value: *value,
                        environment: Some(id),
                    }));
                }
            }
        }
        // A checked generic receiver may specialize to a non-callable value.
        // Its explicit empty binding is distinct from a missing parameter above.
        Ok(origins)
    }

    pub(crate) fn metadata_spec_method_evidence(
        &self,
        invocation: CodeFingerprint,
        environment: CallableEnvironmentId,
    ) -> Result<super::MethodEvidence, EffectSpecializationError> {
        self.dependencies.read(InputFact::MethodEvidence);
        let definition = self
            .method_evidence
            .borrow()
            .get(&invocation)
            .cloned()
            .ok_or_else(|| invalid("metadata method has no evidence definition"))?;
        let snapshot = self.callable_environment(environment)?;
        let mut types = HashMap::new();
        let mut rows = Vec::new();
        for (position, (name, kind)) in definition.generics.iter().enumerate() {
            let value = snapshot
                .generics
                .get(&CallableGenericParameter::Graph {
                    graph: invocation,
                    scope: etas_package_metadata::CallableValueScope::Root,
                    position: position as u32,
                })
                .ok_or_else(|| invalid("metadata method generic environment is incomplete"))?;
            match (kind, value) {
                (CallableGenericParamKind::Type, CallableGenericValue::Type(ty)) => {
                    types.insert(name.clone(), *ty);
                }
                (CallableGenericParamKind::Effect, CallableGenericValue::EffectRow(row)) => {
                    rows.push((name.clone(), row.clone()))
                }
                _ => {
                    return Err(invalid(
                        "metadata method generic environment has inconsistent kinds",
                    ));
                }
            }
        }
        let type_rows = self.type_row_arguments(&rows, Some(snapshot.owner))?;
        let type_parameters = snapshot
            .generics
            .iter()
            .filter_map(|(parameter, value)| match (parameter, value) {
                (
                    CallableGenericParameter::TypeParameter { subject },
                    CallableGenericValue::Type(argument),
                ) => Some((*subject, *argument)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        let mut substitution = self
            .type_substitution(&types)
            .with_type_parameters(&type_parameters)
            .with_effect_arguments(&type_rows);
        let method = &definition.invocation;
        let receiver = substitution.substitute(method.receiver)?;
        let spec_args = method
            .spec_args
            .iter()
            .map(|ty| substitution.substitute(*ty))
            .collect::<Result<Vec<_>, _>>()?;
        let declaration = self
            .types
            .facts
            .external_specs
            .signatures
            .get(&method.spec)
            .ok_or_else(|| invalid("metadata method spec declaration is missing"))?;
        let fact = declaration
            .methods
            .iter()
            .find(|candidate| candidate.name == method.method)
            .ok_or_else(|| invalid("metadata method declaration is missing"))?;
        let signature = fact
            .signature_in(&self.types.facts)
            .ok_or_else(|| invalid("metadata method callable signature is missing"))?;
        let mut arguments = GenericInstantiationFact {
            type_bindings: Vec::new(),
            effect_row_bindings: Vec::new(),
            deferred_effect_row_obligations: Vec::new(),
        };
        let mut actual = method.generic_arguments.iter();
        for (position, parameter) in signature.generic_params.iter().enumerate() {
            if Some(position as u32) == fact.receiver_param {
                continue;
            }
            match (parameter.kind, actual.next()) {
                (
                    CallableGenericParamKind::Type,
                    Some(crate::ExternalSpecMethodGenericArgument::Type(ty)),
                ) => arguments
                    .type_bindings
                    .push((parameter.name.clone(), substitution.substitute(*ty)?)),
                (
                    CallableGenericParamKind::Effect,
                    Some(crate::ExternalSpecMethodGenericArgument::EffectRow(row)),
                ) => arguments.effect_row_bindings.push((
                    parameter.name.clone(),
                    substitution.substitute_effect_row(row.clone())?,
                )),
                _ => {
                    return Err(invalid(
                        "metadata method generic argument kind is inconsistent",
                    ));
                }
            }
        }
        if actual.next().is_some() {
            return Err(invalid("metadata method has extra generic arguments"));
        }
        let implementation = self
            .types
            .facts
            .methods
            .resolve_witness(
                &self.types.store,
                &self.spec_facts,
                receiver,
                &spec_args,
                &fact.identity,
            )
            .map_err(invalid)?;
        if let Some(witness) = implementation.as_ref()
            && let etas_types::method::MethodTarget::Source { callable: item, .. } =
                witness.declaration.target
        {
            self.discovered_spec_unit_targets
                .borrow_mut()
                .entry(snapshot.owner)
                .or_default()
                .insert(crate::EffectUnit::Item(item));
            return self
                .capture_spec_implementation(
                    item,
                    environment,
                    snapshot,
                    signature,
                    fact,
                    super::MethodInstantiation {
                        owner_bindings: witness.owner_bindings.clone(),
                        generics: arguments,
                    },
                )
                .map(super::MethodEvidence::Resolved);
        }
        if let Some(implementation) = implementation.filter(|method| {
            matches!(
                method.declaration.target,
                etas_types::method::MethodTarget::External { .. }
            )
        }) {
            return self
                .capture_external_spec_implementation(
                    implementation.declaration,
                    environment,
                    snapshot,
                    signature,
                    fact,
                    super::MethodInstantiation {
                        owner_bindings: implementation.owner_bindings,
                        generics: arguments,
                    },
                )
                .map(super::MethodEvidence::Resolved);
        }
        if !self.spec_method_has_lexical_parameter(snapshot.owner, receiver, &spec_args)? {
            return Err(invalid(format!(
                "metadata method {}.{} has no unique checked implementation for {:?} with bindings {types:?}",
                method.spec.path.join("."),
                method.method,
                self.types.store.get(receiver),
            )));
        }
        types.extend(declaration.param_names.iter().cloned().zip(spec_args));
        types.extend(arguments.type_bindings);
        if let Some(position) = fact.receiver_param {
            let parameter = signature
                .generic_params
                .get(position as usize)
                .ok_or_else(|| invalid("metadata method receiver binder is invalid"))?;
            types.insert(parameter.name.clone(), receiver);
        }
        let mut substitution = self
            .type_substitution(&types)
            .with_effect_arguments(&type_rows);
        let mut summary = EffectSummary::local();
        if let Some(row) = &signature.effects {
            summary.record_escaping_effect_row(
                self.row_from_type_ref(&substitution.substitute_effect_row(row.clone())?),
            );
        }
        let target = ParameterCallTarget::MetadataSpecMethod {
            invocation,
            environment: Some(environment),
        };
        if etas_types::data_equality::is_pure_collection_member(&self.spec_facts, &fact.identity) {
            super::contract_completion(&mut summary, substitution.substitute(signature.output)?);
            return Ok(super::MethodEvidence::Contract {
                summary: Box::new(summary),
            });
        }
        summary.action_trace = crate::ActionTraceDomain::ParameterCall {
            target,
            parameter: method.method.clone(),
            span: definition.span,
        };
        summary.handler_requests = crate::RequestInstantiationSummary::parameter(
            target,
            method.method.clone(),
            definition.span,
        );
        super::deferred_completion(
            &mut summary,
            target,
            substitution.substitute(signature.output)?,
            definition.span,
        );
        Ok(super::MethodEvidence::Deferred {
            summary: Box::new(summary),
        })
    }
}
