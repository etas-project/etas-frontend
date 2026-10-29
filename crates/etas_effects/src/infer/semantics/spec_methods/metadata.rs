use std::collections::{BTreeMap, HashMap};

use etas_code::CodeFingerprint;
use etas_core::Span;
use etas_types::{
    CallableGenericParamKind, GenericInstantiationFact, TypeId, TypeSubstitutionEngine, TypeUnifier,
};

use crate::{
    EffectRow, EffectSummary, ParameterCallTarget,
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

type GenericArguments<'a> = (&'a [(String, TypeId)], &'a [(String, EffectRow)]);

impl EffectSemantics<'_> {
    pub(crate) fn register_metadata_method(
        &self,
        invocation: &std::sync::Arc<crate::ExternalSpecMethodInvocation>,
        parameters: &[(String, ParameterCallTarget)],
        generics: &[(String, CallableGenericParamKind)],
        span: Span,
    ) -> Result<ParameterCallTarget, EffectSpecializationError> {
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

    pub(crate) fn bind_metadata_method_environment(
        &self,
        invocation: CodeFingerprint,
        environment: Option<CallableEnvironmentId>,
        application: CodeFingerprint,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        generics: GenericArguments<'_>,
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        let (types, rows) = generics;
        if let Some(environment) = environment {
            return self.instantiate_callable_environment_with_values(
                environment,
                application,
                bindings,
                state,
                types,
                rows,
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
            owner,
            parameters: BTreeMap::new(),
            generics: BTreeMap::new(),
        };
        for (position, (name, kind)) in definition.generics.iter().enumerate() {
            let value = match kind {
                CallableGenericParamKind::Type => CallableGenericValue::Type(
                    *types
                        .iter()
                        .find(|(parameter, _)| parameter == name)
                        .map(|(_, ty)| ty)
                        .ok_or_else(|| {
                            invalid(format!("metadata method type binding `{name}` is missing"))
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
        let graph =
            super::metadata_graphs::graph_key(definition.invocation.identity, argument.position);
        let mut captured = CallableEnvironment {
            owner: snapshot.owner,
            generics: BTreeMap::new(),
            parameters: BTreeMap::new(),
        };
        for (parameter, value) in &snapshot.generics {
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
        for (_, parameter) in &definition.parameters {
            let position = match parameter {
                ParameterCallTarget::Interface(position) => *position,
                ParameterCallTarget::Graph { parameter, .. } => parameter.position,
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
                captured.parameters.insert(
                    ParameterCallTarget::Graph {
                        graph,
                        parameter: CallableValueParameter {
                            scope: CallableValueScope::Root,
                            position,
                        },
                    },
                    self.trace_argument_origins(binding, state)?,
                );
            }
        }
        let id = CallableEnvironmentId {
            origin: graph,
            application,
        };
        self.record_callable_environment(id, captured.clone())?;
        let mut origins = std::collections::BTreeSet::new();
        for source in &argument.sources.sources {
            match source {
                CallableValueSource::Parameter(parameter) => {
                    let values = captured
                        .parameters
                        .get(&ParameterCallTarget::Graph {
                            graph,
                            parameter: *parameter,
                        })
                        .ok_or_else(|| {
                            invalid("metadata method callback has no checked argument binding")
                        })?;
                    origins.extend(values);
                }
                CallableValueSource::Value(value) => {
                    origins.insert(LatentSource::Metadata(crate::MetadataCallableSource {
                        graph,
                        value: *value,
                        environment: Some(id),
                    }));
                }
            }
        }
        if origins.is_empty() {
            return Err(invalid(
                "metadata method callback has no checked value sources",
            ));
        }
        Ok(origins)
    }

    pub(crate) fn metadata_spec_method_evidence(
        &self,
        invocation: CodeFingerprint,
        environment: CallableEnvironmentId,
    ) -> Result<super::MethodEvidence, EffectSpecializationError> {
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
        let mut substitution = TypeSubstitutionEngine::existing(&self.types.store, &types)
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
        let same = |left, right| {
            let mut unifier = TypeUnifier::new(&self.types.store);
            unifier.unify(left, right).is_ok() && unifier.substitution().iter().next().is_none()
        };
        let mut targets = self
            .types
            .facts
            .spec_impls
            .iter()
            .filter(|implementation| {
                self.types
                    .facts
                    .external_specs
                    .aliases
                    .get(&implementation.spec_symbol)
                    == Some(&method.spec)
                    && same(implementation.self_type, receiver)
                    && implementation.args.len() == spec_args.len()
                    && implementation
                        .args
                        .iter()
                        .zip(&spec_args)
                        .all(|(left, right)| same(*left, *right))
            })
            .flat_map(|implementation| &implementation.methods)
            .filter(|candidate| candidate.name == method.method)
            .filter_map(
                |candidate| match self.hir.symbols.get(candidate.symbol)?.def {
                    etas_hir::SymbolDef::Item { item }
                        if matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Flow(_))) =>
                    {
                        Some(item)
                    }
                    _ => None,
                },
            )
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        if let [item] = targets.as_slice() {
            return self
                .capture_spec_implementation(
                    *item,
                    environment,
                    snapshot,
                    signature,
                    fact,
                    arguments,
                )
                .map(super::MethodEvidence::Resolved);
        }
        if targets.is_empty()
            && let Some(implementation) = self.external_spec_method(
                &etas_types::CheckedSpecRef::External(method.spec.clone()),
                receiver,
                &spec_args,
                &method.method,
            )?
        {
            return self
                .capture_external_spec_implementation(
                    implementation,
                    environment,
                    snapshot,
                    signature,
                    fact,
                    arguments,
                )
                .map(super::MethodEvidence::Resolved);
        }
        if !targets.is_empty()
            || !self.spec_method_has_lexical_parameter(snapshot.owner, receiver, &spec_args)?
        {
            return Err(invalid(
                "metadata method has no unique checked implementation",
            ));
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
        let mut substitution = TypeSubstitutionEngine::existing(&self.types.store, &types)
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
