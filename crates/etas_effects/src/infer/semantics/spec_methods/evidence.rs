use std::collections::{BTreeMap, HashMap};

use etas_hir::{HirArg, HirExpr, HirExprId, HirItemId, ResolveResult};
use etas_types::{CallableGenericParamKind, TypeId, TypeSubstitutionEngine};

use crate::{
    ActionTraceDomain, EffectRow, EffectSet, EffectSummary, ParameterCallTarget,
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
    Deferred { summary: Box<EffectSummary> },
}

impl EffectSemantics<'_> {
    pub(crate) fn bind_spec_method_arguments(
        &self,
        summary: &EffectSummary,
        bindings: &mut TraceArgumentBindings,
        application: etas_code::CodeFingerprint,
        state: &EffectState,
        types: &[(String, TypeId)],
        rows: &[(String, EffectRow)],
    ) -> Result<(), EffectSpecializationError> {
        let mut calls = BTreeMap::new();
        summary.action_trace.collect_parameter_calls(&mut calls);
        if let Some(exits) = &summary.exits {
            for exit in &exits.exits {
                exit.trace.collect_parameter_calls(&mut calls);
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
            if let ParameterCallTarget::MetadataSpecMethod {
                invocation,
                environment,
            } = reference.target
            {
                let environment = self.bind_metadata_method_environment(
                    invocation,
                    environment,
                    application,
                    bindings,
                    state,
                    (types, rows),
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
                types,
                rows,
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
        let parameters = self
            .native_generic_params(owner)
            .ok_or_else(|| invalid("spec invocation has no checked generic scope"))?;
        let generics = parameters
            .iter()
            .enumerate()
            .map(|(position, parameter)| {
                let value = match parameter.kind {
                    CallableGenericParamKind::Type => CallableGenericValue::Type(parameter.subject),
                    CallableGenericParamKind::Effect => {
                        CallableGenericValue::EffectRow(EffectRow {
                            effects: EffectSet::new(),
                            open: Some(crate::effect_var_id_from_name(&parameter.name)),
                        })
                    }
                };
                (
                    CallableGenericParameter::Native {
                        owner,
                        position: position as u32,
                    },
                    value,
                )
            })
            .collect();
        let Some(HirExpr::SpecMethodCall { receiver, args, .. }) = self.hir.exprs.get(call) else {
            return Err(invalid("spec invocation is missing its checked expression"));
        };
        let arguments = std::iter::once(*receiver).chain(args.iter().map(|arg| match arg {
            HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
        }));
        let mut captures = BTreeMap::new();
        for (position, expr) in arguments.enumerate() {
            if !self.expr_type_contains_flow(expr) {
                continue;
            }
            let sources = self
                .latent_sources_for_expr(state, expr)
                .ok_or_else(|| invalid("spec invocation callback has no checked value sources"))?;
            captures.insert(
                ParameterCallTarget::Interface(position as u32),
                sources.into_iter().collect(),
            );
        }
        let site = self.callable_environment_site(call)?;
        let environment = CallableEnvironmentId {
            origin: site,
            application: site,
        };
        self.record_callable_environment(
            environment,
            CallableEnvironment {
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
        let Some(HirExpr::SpecMethodCall {
            receiver,
            spec_path,
            spec_args,
            method,
            ..
        }) = self.hir.exprs.get(call)
        else {
            return Err(invalid("method evidence has no checked selection"));
        };
        let ResolveResult::Resolved(spec) = spec_path.resolution else {
            return Err(invalid("method evidence has no resolved spec"));
        };
        let spec = self
            .type_symbols
            .canonical_symbol(self.hir, spec)
            .ok_or_else(|| invalid("method evidence has no canonical spec identity"))?;
        let lexical_owner = *self
            .context
            .view()
            .index()
            .expr_item
            .get(&call)
            .ok_or_else(|| invalid("method evidence has no checked lexical owner"))?;
        let snapshot = self.callable_environment(environment)?;
        let (types, rows) = self.native_environment_generics(lexical_owner, &snapshot.generics)?;
        let names = types.iter().cloned().collect::<HashMap<_, _>>();
        let type_rows = self.type_row_arguments(&rows, Some(snapshot.owner))?;
        let mut substitution = TypeSubstitutionEngine::existing(&self.types.store, &names)
            .with_effect_arguments(&type_rows);
        let receiver = *self
            .types
            .facts
            .expr_types
            .get(receiver)
            .ok_or_else(|| invalid("method evidence receiver has no checked type"))?;
        let receiver = substitution.substitute(receiver)?;
        let args = spec_args
            .iter()
            .map(|arg| {
                let ty =
                    *self.types.facts.type_refs.get(arg).ok_or_else(|| {
                        invalid("method evidence spec argument has no checked type")
                    })?;
                substitution
                    .substitute(ty)
                    .map_err(EffectSpecializationError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signature = self
            .types
            .facts
            .spec_signatures
            .get(&spec)
            .ok_or_else(|| invalid("method evidence spec signature is missing"))?;
        let method_fact = signature
            .methods
            .iter()
            .find(|candidate| candidate.name == *method)
            .ok_or_else(|| invalid("method evidence declaration is missing"))?;
        let declaration = method_fact
            .signature_in(&self.types.facts)
            .ok_or_else(|| invalid("method evidence callable signature is missing"))?;
        let mut arguments = self.call_generic_bindings(call);
        for (_, ty) in &mut arguments.types {
            *ty = substitution.substitute(*ty)?;
        }
        for (_, row) in &mut arguments.effect_rows {
            *row = substitution.substitute_effect_row(row.clone())?;
        }
        self.solve_spec_method_row_obligations(call, &snapshot, &mut arguments)?;
        if let Some(EffectUnit::Item(item)) =
            self.spec_method_target_for_types(spec, receiver, &args, method)
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
                    etas_types::GenericInstantiationFact {
                        type_bindings: arguments.types,
                        effect_row_bindings: arguments.effect_rows,
                        deferred_effect_row_obligations: Vec::new(),
                    },
                )
                .map(MethodEvidence::Resolved);
        }
        if let Some(implementation) = self.external_spec_method(
            &etas_types::CheckedSpecRef::Source(spec),
            receiver,
            &args,
            method,
        )? {
            return self
                .capture_external_spec_implementation(
                    implementation,
                    environment,
                    snapshot,
                    declaration,
                    method_fact,
                    etas_types::GenericInstantiationFact {
                        type_bindings: arguments.types,
                        effect_row_bindings: arguments.effect_rows,
                        deferred_effect_row_obligations: Vec::new(),
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
        bound_bindings.extend(signature.param_names.iter().cloned().zip(args));
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
        let mut substitution = TypeSubstitutionEngine::existing(&self.types.store, &bound_bindings)
            .with_effect_arguments(&type_rows);
        let mut summary = EffectSummary::local();
        if let Some(row) = &declaration.effects {
            let row = substitution.substitute_effect_row(row.clone())?;
            summary.record_escaping_effect_row(self.row_from_type_ref(&row));
        }
        let span = self.hir.exprs[call].span(&self.hir.blocks);
        let target = ParameterCallTarget::SpecMethod { call, environment };
        summary.action_trace = ActionTraceDomain::ParameterCall {
            target,
            parameter: method.clone(),
            span,
        };
        summary.handler_requests =
            crate::RequestInstantiationSummary::parameter(target, method.clone(), span);
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
        for parameter in self
            .native_generic_params(owner)
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
        arguments: etas_types::GenericInstantiationFact,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let Some(etas_types::ItemSignature::Flow(implementation)) =
            self.types.facts.item_signatures.get(&item)
        else {
            return Err(invalid("method implementation has no checked signature"));
        };
        let arguments = method
            .implementation_instantiation(declaration, implementation, &arguments)
            .map_err(|reason| invalid(&reason))?;
        let mut generics = BTreeMap::new();
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
