use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_hir::{HirExprId, HirItemId};
use etas_types::{CallableGenericParamKind, TypeId};

use crate::{
    ActionTraceDomain, EffectSummary, ParameterCallTarget,
    facts::{
        CallableEnvironment, CallableEnvironmentId, CallableGenericParameter, CallableGenericValue,
    },
    infer::semantics::{
        engine::EffectSemantics,
        specialize::{EffectSpecializationError, TraceArgumentBindings},
        state::{EffectState, LatentSource},
    },
    infer::unit::EffectUnit,
};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

pub(crate) enum MethodEvidence {
    Resolved(LatentSource),
    Alternatives(std::collections::BTreeSet<LatentSource>),
    /// Execution behavior is fixed by a validated standard member contract.
    /// The value observation remains unknown and implementation dispatch stays typed.
    Contract {
        summary: Box<EffectSummary>,
    },
    Deferred {
        summary: Box<EffectSummary>,
    },
}

impl EffectSemantics<'_> {
    pub(in crate::infer::semantics) fn bind_spec_method_arguments(
        &self,
        summary: &EffectSummary,
        bindings: &mut TraceArgumentBindings,
        application: etas_code::CodeFingerprint,
        state: &EffectState,
        generics: super::super::callable_generics::ScopedGenericArguments<'_>,
        formal_parameters: &mut std::collections::BTreeSet<ParameterCallTarget>,
        resolve_captures: bool,
    ) -> Result<(), EffectSpecializationError> {
        let mut calls = BTreeMap::new();
        summary.action_trace.collect_parameter_calls(&mut calls);
        if let Some(exits) = &summary.exits {
            for exit in &exits.exits {
                exit.trace.collect_parameter_calls(&mut calls);
            }
            for exit in &exits.parameter_exits {
                exit.prefix.collect_parameter_calls(&mut calls);
            }
            // Completion identifies the called value separately from the
            // invocation projection retained by its execution trace.
            for gap in &exits.gaps {
                if let crate::OutcomeAnalysisGap::ParameterCall { target, source, .. } = gap {
                    let span = source.span();
                    calls
                        .entry((*target, span.source.0, span.range.start.0, span.range.end.0))
                        .or_insert(crate::infer::domain::ParameterCallRef {
                            target: *target,
                            parameter: "callback completion".into(),
                            span,
                        });
                }
            }
        }
        if let crate::RequestInstantiationSummary::Available { parameters, .. } =
            &summary.handler_requests
        {
            for request in parameters {
                calls
                    .entry((
                        request.target,
                        request.span.source.0,
                        request.span.range.start.0,
                        request.span.range.end.0,
                    ))
                    .or_insert(crate::infer::domain::ParameterCallRef {
                        target: request.target,
                        parameter: request.parameter.clone(),
                        span: request.span,
                    });
            }
        }
        for reference in calls.into_values() {
            if let Some((root, path)) = self.parameter_projection_target(reference.target) {
                let supplied = self.has_projection_binding(reference.target, bindings);
                let caller_parameter = matches!(root, ParameterCallTarget::Native(symbol)
                    if matches!(self.hir.symbols.get(symbol).map(|symbol| &symbol.def),
                        Some(etas_hir::SymbolDef::Param { owner, .. }) if Some(*owner) == state.owner));
                if !supplied && (formal_parameters.contains(&root) || caller_parameter) {
                    // The callback can remain a lexical parameter while its
                    // invocation inputs belong to this instantiated frame.
                    // Substitute those inputs before exporting its summary.
                    if let ParameterCallTarget::Projection(id) = reference.target {
                        let mut selected = path.clone();
                        self.bind_projection_inputs(&mut selected, bindings, state)?;
                        if selected != path {
                            let projection = self.parameter_projection(id).ok_or_else(|| {
                                invalid("symbolic invocation has no checked projection")
                            })?;
                            let projected = self.intern_parameter_projection(
                                projection.owner,
                                projection.parameter,
                                selected,
                            )?;
                            bindings.insert(
                                reference.target,
                                super::super::specialize::TraceArgumentBinding::Origins(
                                    [LatentSource::Projection(projected)].into(),
                                ),
                            );
                            continue;
                        }
                    }
                    formal_parameters.insert(reference.target);
                    continue;
                }
                if supplied
                    && (resolve_captures
                        || path.iter().any(|step| {
                            matches!(
                                step,
                                etas_code::descriptor::ValueProjectionStep::Invocation { .. }
                            )
                        }))
                    && let Some(binding) = self.trace_argument(
                        reference.target,
                        &reference.parameter,
                        bindings,
                        state,
                    )?
                {
                    let origins = self.trace_argument_origins(&binding, state)?;
                    bindings.insert(
                        reference.target,
                        super::super::specialize::TraceArgumentBinding::Origins(origins),
                    );
                }
                continue;
            }
            if let ParameterCallTarget::MetadataSpecMethod {
                invocation,
                environment,
            } = reference.target
            {
                let definition = self
                    .method_evidence
                    .borrow()
                    .get(&invocation)
                    .cloned()
                    .ok_or_else(|| invalid("metadata method has no checked evidence definition"))?;
                let mut missing = definition
                    .parameters
                    .iter()
                    .map(|(_, parameter)| parameter)
                    .filter(|parameter| !bindings.contains_key(parameter));
                if let Some(first) = missing.next()
                    && formal_parameters.contains(first)
                    && missing.all(|parameter| formal_parameters.contains(parameter))
                {
                    // A first-class callable still owns these formal inputs.
                    // Its caller binds the method evidence when supplying them.
                    formal_parameters.insert(reference.target);
                    continue;
                }
                let environment = self.bind_metadata_method_environment(
                    invocation,
                    environment,
                    application,
                    bindings,
                    state,
                    generics,
                )?;
                bindings.insert(
                    reference.target,
                    super::super::specialize::TraceArgumentBinding::Origins(
                        [LatentSource::MetadataSpecMethod {
                            invocation,
                            environment,
                        }]
                        .into(),
                    ),
                );
                continue;
            }
            let ParameterCallTarget::SpecMethod { call, environment } = reference.target else {
                continue;
            };
            let environment = self.instantiate_callable_environment_with_values(
                environment,
                application,
                bindings,
                state,
                generics,
            )?;
            bindings.insert(
                reference.target,
                super::super::specialize::TraceArgumentBinding::Origins(
                    [LatentSource::SpecMethod { call, environment }].into(),
                ),
            );
        }
        Ok(())
    }

    pub(crate) fn spec_method_call_evidence(
        &self,
        call: HirExprId,
        state: &EffectState,
    ) -> Result<super::MethodEvidence, EffectSpecializationError> {
        let owner = state
            .owner
            .ok_or_else(|| invalid("spec invocation has no checked owner"))?;
        let generics = self.lexical_generic_values(owner)?;
        let arguments = self.checked_method_argument_slots(call)?;
        let mut required_paths = BTreeMap::<u32, BTreeSet<_>>::new();
        if let Some(fact) = self.types.facts.checked_method_calls.get(&call)
            && let etas_types::method::MethodTarget::External { identity } = &fact.target
        {
            // A generic implementation can return a projection even when the
            // caller's instantiation gives it an empty callable payload.
            let (implementation, root) = self.external_method_graph(identity)?;
            for parameter in implementation.graph.parameter_references() {
                if parameter.scope == etas_package_metadata::CallableValueScope::Value(root)
                    && parameter.projection.is_some()
                {
                    let path = implementation
                        .graph
                        .projection_path(parameter)
                        .map_err(|error| invalid(&error.to_string()))?;
                    let structural = path
                        .iter()
                        .take_while(|step| {
                            etas_code::descriptor::ValueProjectionStep::call_inputs(
                                std::slice::from_ref(step),
                            )
                            .is_empty()
                                && !matches!(
                                    step,
                                    etas_code::descriptor::ValueProjectionStep::CallResult
                                )
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if !structural.is_empty() {
                        required_paths
                            .entry(parameter.position)
                            .or_default()
                            .insert(structural);
                    }
                }
            }
        }
        let mut captures = BTreeMap::new();
        for (position, expr) in arguments.into_iter().enumerate() {
            let position = position as u32;
            let Some(expr) = expr else {
                if let Some(binding) =
                    self.default_argument_binding(call, position as usize, state)?
                {
                    captures.insert(
                        ParameterCallTarget::Interface(position),
                        self.trace_argument_origins(&binding, state)?,
                    );
                    let mut paths = required_paths.remove(&position).unwrap_or_default();
                    if let super::super::specialize::TraceArgumentBinding::InputData {
                        argument,
                        ..
                    } = &binding
                    {
                        paths.extend(
                            argument
                                .projections
                                .iter()
                                .map(|projection| projection.path.clone()),
                        );
                    }
                    for path in paths {
                        let mut projected = binding.clone();
                        if let super::super::specialize::TraceArgumentBinding::InputData {
                            path: selected,
                            ..
                        } = &mut projected
                        {
                            *selected = path.clone();
                        }
                        let sources = self.trace_argument_origins(&projected, state)?;
                        let target = self
                            .intern_external_parameter_projection(
                                ParameterCallTarget::Interface(position),
                                path,
                            )
                            .ok_or_else(|| {
                                invalid(
                                    "default argument projection exceeds its checked graph budget",
                                )
                            })?;
                        captures.insert(target, sources);
                    }
                    continue;
                }
                let parameter = self.types.facts.checked_method_calls[&call]
                    .signature
                    .params[position as usize];
                let paths = self
                    .value_payload_paths(parameter)
                    .ok_or_else(|| invalid("default argument has no checked payload shape"))?;
                if !paths.is_empty() {
                    return Err(invalid(
                        "foreign default callback requires its checked callable behavior",
                    ));
                }
                // The default is evaluated in its defining revision. A checked
                // data-only parameter has no callable origins to import into
                // the caller, just like an explicitly supplied scalar value.
                captures.insert(ParameterCallTarget::Interface(position), BTreeSet::new());
                for path in required_paths.remove(&position).unwrap_or_default() {
                    let target = self
                        .intern_external_parameter_projection(
                            ParameterCallTarget::Interface(position),
                            path,
                        )
                        .ok_or_else(|| {
                            invalid("default argument projection exceeds its checked graph budget")
                        })?;
                    captures.insert(target, BTreeSet::new());
                }
                continue;
            };
            let sources =
                if self.expr_type_contains_flow(expr) || self.symbolic_contract_expr(state, expr) {
                    self.latent_sources_for_expr(state, expr).ok_or_else(|| {
                        invalid("spec invocation callback has no checked value sources")
                    })?
                } else {
                    // A checked scalar supplies an empty callable payload. It is
                    // still an argument when the method captures a generic input.
                    Vec::new()
                };
            captures.insert(
                ParameterCallTarget::Interface(position),
                sources.into_iter().collect(),
            );
            let mut paths = required_paths.remove(&position).unwrap_or_default();
            if let Some(payload_paths) = self
                .types
                .facts
                .expr_types
                .get(&expr)
                .and_then(|ty| self.value_payload_paths(*ty))
            {
                paths.extend(payload_paths);
            }
            for path in paths {
                let sources = self
                    .projected_sources(expr, &path, state)
                    .unwrap_or_else(|| [LatentSource::Unavailable { expr }].into());
                let target = self
                    .intern_external_parameter_projection(
                        ParameterCallTarget::Interface(position),
                        path,
                    )
                    .ok_or_else(|| {
                        invalid("spec argument projection exceeds its checked graph budget")
                    })?;
                captures.insert(target, sources);
            }
        }
        let site = self.callable_environment_site(call)?;
        let environment = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&generics),
            instance: None,
            scope: Some(self.callable_environment_scope(owner)?),
            origin: site,
            application: site,
        };
        self.record_callable_environment(
            environment,
            CallableEnvironment {
                invocation_origin: None,
                invocation_inputs: Default::default(),
                owner,
                generics,
                parameters: captures,
            },
        )?;
        self.spec_method_evidence(call, environment)
    }

    pub(crate) fn spec_method_evidence(
        &self,
        call: HirExprId,
        environment: CallableEnvironmentId,
    ) -> Result<MethodEvidence, EffectSpecializationError> {
        let fact = self
            .types
            .facts
            .checked_method_calls
            .get(&call)
            .ok_or_else(|| invalid("method evidence has no checked call fact"))?;
        let lexical_owner = *self
            .context
            .view()
            .index()
            .expr_item
            .get(&call)
            .ok_or_else(|| invalid("method evidence has no checked lexical owner"))?;
        let snapshot = self.callable_environment(environment)?;
        let (types, rows) = self.native_environment_generics(lexical_owner, &snapshot.generics)?;
        let type_parameters =
            self.native_environment_type_parameters(lexical_owner, &snapshot.generics)?;
        let names = types.iter().cloned().collect::<HashMap<_, _>>();
        let type_rows = self.type_row_arguments(&rows, Some(snapshot.owner))?;
        let mut substitution = self
            .type_substitution(&names)
            .with_type_parameters(&type_parameters)
            .with_effect_arguments(&type_rows);
        let mut arguments = self.call_generic_bindings(call);
        for (_, ty) in &mut arguments.types {
            *ty = substitution.substitute(*ty)?;
        }
        for (_, row) in &mut arguments.effect_rows {
            *row = substitution.substitute_effect_row(row.clone())?;
        }
        self.solve_spec_method_row_obligations(call, &snapshot, &mut arguments)?;
        if let etas_types::method::MethodTarget::Source { callable, .. } = fact.target {
            let owner_bindings = fact
                .owner_bindings
                .iter()
                .map(|(parameter, argument)| Ok((*parameter, substitution.substitute(*argument)?)))
                .collect::<Result<_, EffectSpecializationError>>()?;
            return self
                .capture_source_method_implementation(
                    callable,
                    environment,
                    snapshot,
                    super::MethodInstantiation {
                        owner_bindings,
                        generics: etas_types::GenericInstantiationFact {
                            type_bindings: arguments.types,
                            effect_row_bindings: arguments.effect_rows,
                            deferred_effect_row_obligations: Vec::new(),
                        },
                    },
                )
                .map(MethodEvidence::Resolved);
        }
        if let etas_types::method::MethodTarget::External { identity } = &fact.target {
            let owner_bindings = fact
                .owner_bindings
                .iter()
                .map(|(parameter, argument)| Ok((*parameter, substitution.substitute(*argument)?)))
                .collect::<Result<_, EffectSpecializationError>>()?;
            return self
                .capture_external_method(
                    identity,
                    &fact.signature,
                    environment,
                    snapshot,
                    &owner_bindings,
                    (
                        arguments.types,
                        arguments
                            .effect_rows
                            .iter()
                            .map(|(name, row)| (name.clone(), self.row_from_type_ref(row)))
                            .collect(),
                    ),
                )
                .map(MethodEvidence::Resolved);
        }
        let etas_types::method::MethodTarget::BoundSpec { spec, member, .. } = &fact.target else {
            return Err(invalid(
                "method evidence has no checked bound witness recipe",
            ));
        };
        let etas_types::method::MethodEvidence::Bound {
            args: spec_args, ..
        } = &fact.evidence
        else {
            return Err(invalid("method witness has no checked spec arguments"));
        };
        let receiver = substitution.substitute(fact.receiver_type)?;
        let args = spec_args
            .iter()
            .map(|arg| {
                substitution
                    .substitute(*arg)
                    .map_err(EffectSpecializationError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (parameter_names, members) = match spec {
            etas_types::CheckedSpecRef::Source(symbol) => self
                .type_symbols
                .canonical_symbol(self.hir, *symbol)
                .and_then(|symbol| self.types.facts.spec_signatures.get(&symbol))
                .map(|spec| (&spec.param_names, &spec.methods)),
            etas_types::CheckedSpecRef::External(identity) => self
                .types
                .facts
                .external_specs
                .signatures
                .get(identity)
                .map(|spec| (&spec.param_names, &spec.methods)),
            _ => None,
        }
        .ok_or_else(|| invalid("method evidence spec signature is missing"))?;
        let method_fact = members
            .iter()
            .find(|candidate| candidate.identity == *member)
            .ok_or_else(|| invalid("method evidence declaration is missing"))?;
        let declaration = method_fact
            .signature_in(&self.types.facts)
            .ok_or_else(|| invalid("method evidence callable signature is missing"))?;
        if matches!(
            self.types.store.get(receiver),
            Some(etas_types::Type::Existential { .. })
        ) && let Some(sources) =
            self.existential_method_sources(super::objects::ObjectMethodInput {
                call,
                invocation: environment,
                snapshot: &snapshot,
                method: method_fact,
                declaration,
                arguments: &arguments,
                spec_args: &args,
            })?
        {
            return Ok(MethodEvidence::Alternatives(sources));
        }
        let witness = self
            .types
            .facts
            .methods
            .resolve_witness(&self.types.store, &self.spec_facts, receiver, &args, member)
            .map_err(|reason| invalid(&reason))?;
        if let Some(witness) = witness.as_ref()
            && let etas_types::method::MethodTarget::Source { callable: item, .. } =
                witness.declaration.target
        {
            self.discovered_spec_targets
                .borrow_mut()
                .entry(call)
                .or_default()
                .insert(EffectUnit::Item(item));
            return self
                .capture_spec_implementation(
                    item,
                    environment,
                    snapshot,
                    declaration,
                    method_fact,
                    super::MethodInstantiation {
                        owner_bindings: witness.owner_bindings.clone(),
                        generics: etas_types::GenericInstantiationFact {
                            type_bindings: arguments.types,
                            effect_row_bindings: arguments.effect_rows,
                            deferred_effect_row_obligations: Vec::new(),
                        },
                    },
                )
                .map(MethodEvidence::Resolved);
        }
        if let Some(witness) = witness.filter(|witness| {
            matches!(
                witness.declaration.target,
                etas_types::method::MethodTarget::External { .. }
            )
        }) {
            return self
                .capture_external_spec_implementation(
                    witness.declaration,
                    environment,
                    snapshot,
                    declaration,
                    method_fact,
                    super::MethodInstantiation {
                        owner_bindings: witness.owner_bindings,
                        generics: etas_types::GenericInstantiationFact {
                            type_bindings: arguments.types,
                            effect_row_bindings: arguments.effect_rows,
                            deferred_effect_row_obligations: Vec::new(),
                        },
                    },
                )
                .map(MethodEvidence::Resolved);
        }
        if !self.spec_method_has_lexical_parameter(snapshot.owner, receiver, &args)? {
            return Err(invalid(
                "method evidence has no unique checked implementation",
            ));
        }
        let mut bound_bindings = names;
        bound_bindings.extend(parameter_names.iter().cloned().zip(args));
        bound_bindings.extend(arguments.types);
        let mut type_rows = type_rows;
        type_rows.extend(arguments.effect_rows);
        if let Some(index) = method_fact.receiver_param {
            let parameter = declaration
                .generic_params
                .get(index as usize)
                .ok_or_else(|| invalid("method evidence receiver binder is invalid"))?;
            bound_bindings.insert(parameter.name.clone(), receiver);
        }
        let mut substitution = self
            .type_substitution(&bound_bindings)
            .with_type_parameters(&type_parameters)
            .with_effect_arguments(&type_rows);
        let mut summary = EffectSummary::local();
        if let Some(row) = &declaration.effects {
            let row = substitution.substitute_effect_row(row.clone())?;
            summary.record_escaping_effect_row(self.row_from_type_ref(&row));
        }
        let span = self.hir.exprs[call].span(&self.hir.blocks);
        let target = ParameterCallTarget::SpecMethod { call, environment };
        if etas_types::data_equality::is_hash_member(&self.spec_facts, &method_fact.identity) {
            // Every implementation is checked against the standard Hash
            // execution contract. It adds no action or request obligation.
            super::contract_completion(&mut summary, substitution.substitute(declaration.output)?);
            return Ok(MethodEvidence::Contract {
                summary: Box::new(summary),
            });
        }
        summary.action_trace = ActionTraceDomain::ParameterCall {
            target,
            parameter: method_fact.name.clone(),
            span,
        };
        summary.handler_requests =
            crate::RequestInstantiationSummary::parameter(target, method_fact.name.clone(), span);
        super::deferred_completion(
            &mut summary,
            target,
            substitution.substitute(declaration.output)?,
            span,
        );
        Ok(MethodEvidence::Deferred {
            summary: Box::new(summary),
        })
    }

    pub(super) fn spec_method_has_lexical_parameter(
        &self,
        owner: HirItemId,
        receiver: TypeId,
        args: &[TypeId],
    ) -> Result<bool, EffectSpecializationError> {
        if matches!(
            self.types.store.get(receiver),
            Some(etas_types::Type::Existential { .. })
        ) {
            return Ok(true);
        }
        for (_, parameter) in self
            .native_generic_interface(owner)
            .ok_or_else(|| invalid("method evidence lexical scope is missing"))?
        {
            if let Some(etas_types::Type::Named(named)) = self.types.store.get(parameter.subject)
                && let Some(symbol) = named.source_parameter
            {
                for ty in std::iter::once(receiver).chain(args.iter().copied()) {
                    if etas_types::type_contains_source_parameter(&self.types.store, ty, symbol)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub(super) fn capture_spec_implementation(
        &self,
        item: HirItemId,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        declaration: &etas_types::CallableSignature,
        method: &etas_types::SpecMethodFact,
        arguments: super::MethodInstantiation,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let Some(etas_types::ItemSignature::Flow(implementation)) =
            self.types.facts.item_signatures.get(&item)
        else {
            return Err(invalid("method implementation has no checked signature"));
        };
        let arguments_owner = arguments.owner_bindings;
        let arguments = method
            .implementation_instantiation(declaration, implementation, &arguments.generics)
            .map_err(|reason| invalid(&reason))?;
        self.capture_source_method_implementation(
            item,
            invocation,
            snapshot,
            super::MethodInstantiation {
                generics: arguments,
                owner_bindings: arguments_owner,
            },
        )
    }

    pub(in crate::infer::semantics) fn capture_source_method_implementation(
        &self,
        item: HirItemId,
        invocation: CallableEnvironmentId,
        snapshot: CallableEnvironment,
        arguments: super::MethodInstantiation,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let Some(etas_types::ItemSignature::Flow(implementation)) =
            self.types.facts.item_signatures.get(&item)
        else {
            return Err(invalid("method implementation has no checked signature"));
        };
        let mut generics = BTreeMap::new();
        if let Some(method) = self.native_impl_method(item) {
            for (position, parameter) in method.owner_params.iter().enumerate() {
                let argument = arguments
                    .owner_bindings
                    .get(&parameter.subject)
                    .ok_or_else(|| invalid("source method owner binding is missing"))?;
                generics.insert(
                    CallableGenericParameter::Native {
                        owner: method.implementation,
                        position: position as u32,
                    },
                    CallableGenericValue::Type(*argument),
                );
            }
        }
        let arguments = arguments.generics;
        for (position, parameter) in implementation.generic_params.iter().enumerate() {
            let value = match parameter.kind {
                CallableGenericParamKind::Type => CallableGenericValue::Type(
                    *arguments
                        .type_bindings
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                        .map(|(_, ty)| ty)
                        .ok_or_else(|| invalid("method type argument is missing"))?,
                ),
                CallableGenericParamKind::Effect => CallableGenericValue::EffectRow(
                    self.row_from_type_ref(
                        arguments
                            .effect_row_bindings
                            .iter()
                            .find(|(name, _)| name == &parameter.name)
                            .map(|(_, row)| row)
                            .ok_or_else(|| invalid("method row argument is missing"))?,
                    ),
                ),
            };
            generics.insert(
                CallableGenericParameter::Native {
                    owner: item,
                    position: position as u32,
                },
                value,
            );
        }
        let params = self
            .unit_params(EffectUnit::Item(item))
            .ok_or_else(|| invalid("method implementation parameters are missing"))?;
        let mut parameters = BTreeMap::new();
        for (position, symbol) in params.iter().enumerate() {
            if let Some(origins) = snapshot
                .parameters
                .get(&ParameterCallTarget::Interface(position as u32))
            {
                parameters.insert(ParameterCallTarget::Native(*symbol), origins.clone());
            }
        }
        let environment = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&generics),
            instance: None,
            scope: Some(self.callable_environment_scope(snapshot.owner)?),
            origin: etas_code::CodeFingerprint::of_parts(
                "etas.effects.spec-method-implementation.v1",
                [
                    invocation.origin.as_bytes().as_slice(),
                    item.0.to_le_bytes().as_slice(),
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
        Ok(LatentSource::Captured {
            unit: EffectUnit::Item(item),
            environment,
        })
    }
}
