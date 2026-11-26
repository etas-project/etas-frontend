use std::collections::BTreeMap;

use super::super::projected_returns::element_origin_path;
use super::EffectSpecializationError;
use crate::{
    ParameterCallTarget,
    infer::semantics::{engine::EffectSemantics, shared::*},
};

#[derive(Clone, serde::Serialize)]
pub(crate) enum TraceArgumentBinding {
    Expression(HirExprId),
    Origins(std::collections::BTreeSet<LatentSource>),
    InputData {
        argument: std::sync::Arc<crate::facts::NativeCallArgument>,
        path: Vec<etas_code::descriptor::ValueProjectionStep>,
        call: HirExprId,
    },
    ProjectedExpression {
        expr: HirExprId,
        path: Vec<etas_code::descriptor::ValueProjectionStep>,
    },
    ProjectedOrigins {
        sources: std::collections::BTreeSet<LatentSource>,
        path: Vec<etas_code::descriptor::ValueProjectionStep>,
    },
    AgentTools(TypeId),
}

impl TraceArgumentBinding {
    fn pending_origins(&self) -> bool {
        matches!(self, Self::Origins(sources) if sources.iter().any(|source| matches!(source, LatentSource::Unavailable { .. })))
    }

    fn symbolic_origins(&self) -> bool {
        matches!(self, Self::Origins(sources) if !sources.is_empty() && sources.iter().all(|source| matches!(source,
            LatentSource::Unavailable { .. } | LatentSource::Parameter { .. } | LatentSource::Projection(_))))
    }
}

pub(crate) type TraceArgumentBindings = BTreeMap<ParameterCallTarget, TraceArgumentBinding>;

/// Derivative of a checked regular value path at a captured position.
/// (P A)* P = P (A P)*, including unions sharing the prefix P.
fn projected_suffix(
    path: &[etas_code::descriptor::ValueProjectionStep],
    prefix: &[etas_code::descriptor::ValueProjectionStep],
) -> Option<Vec<etas_code::descriptor::ValueProjectionStep>> {
    use etas_code::descriptor::ValueProjectionStep as Step;
    if let Some(suffix) = path.strip_prefix(prefix) {
        return Some(suffix.to_vec());
    }
    let [Step::Repeat { alternatives }, rest @ ..] = path else {
        return None;
    };
    let rest = rest.strip_prefix(prefix)?;
    let alternatives = alternatives
        .iter()
        .map(|alternative| {
            let mut rotated = alternative.strip_prefix(prefix)?.to_vec();
            rotated.extend_from_slice(prefix);
            Some(rotated)
        })
        .collect::<Option<std::collections::BTreeSet<_>>>()?;
    let mut suffix = vec![Step::Repeat {
        alternatives: alternatives.into_iter().collect(),
    }];
    suffix.extend_from_slice(rest);
    Some(suffix)
}

impl EffectSemantics<'_> {
    pub(in crate::infer::semantics) fn has_projection_binding(
        &self,
        target: ParameterCallTarget,
        bindings: &TraceArgumentBindings,
    ) -> bool {
        if bindings.contains_key(&target) {
            return true;
        }
        let Some((root, path)) = self.parameter_projection_target(target) else {
            return false;
        };
        bindings.contains_key(&root)
            || bindings.keys().any(|candidate| {
                self.parameter_projection_target(*candidate)
                    .is_some_and(|(base, selected)| {
                        base == root && projected_suffix(&path, &selected).is_some()
                    })
            })
    }

    pub(crate) fn specialize_parameter_call_trace(
        &self,
        trace: &ActionTraceDomain,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<ActionTraceDomain, EffectSpecializationError> {
        super::substitute_parameter_calls(trace, |target, label, span| {
            let Some(binding) = self.trace_argument(target, label, bindings, state)? else {
                return Ok(super::ParameterCallSubstitution::Symbolic);
            };
            Ok(super::ParameterCallSubstitution::Trace(
                self.trace_argument_summary(&binding, state, label, span)?
                    .action_trace,
            ))
        })
    }

    pub(in crate::infer::semantics) fn concrete_application_base<'a>(
        &self,
        target: ParameterCallTarget,
        bindings: impl IntoIterator<
            Item = (
                ParameterCallTarget,
                &'a std::collections::BTreeSet<LatentSource>,
            ),
        >,
    ) -> bool {
        use etas_code::descriptor::ValueProjectionStep as Step;
        let Some((root, path)) = self.parameter_projection_target(target) else {
            return false;
        };
        if Step::call_inputs(&path).is_empty() {
            return false;
        }
        bindings.into_iter().any(|(candidate, sources)| {
            if sources.is_empty()
                || sources.iter().any(|source| {
                    matches!(
                        source,
                        LatentSource::Unavailable { .. }
                            | LatentSource::Parameter { .. }
                            | LatentSource::Projection(_)
                    )
                })
            {
                return false;
            }
            let prefix = if candidate == root {
                Vec::new()
            } else {
                let Some((base, prefix)) = self.parameter_projection_target(candidate) else {
                    return false;
                };
                if base != root {
                    return false;
                }
                prefix
            };
            projected_suffix(&path, &prefix).is_some_and(|suffix| {
                matches!(
                    suffix.first(),
                    Some(
                        Step::Invocation { .. } | Step::CallWithArguments { .. } | Step::CallResult
                    )
                )
            })
        })
    }

    pub(crate) fn trace_argument(
        &self,
        target: ParameterCallTarget,
        label: &str,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<Option<TraceArgumentBinding>, EffectSpecializationError> {
        let reselect = self
            .parameter_projection_target(target)
            .is_some_and(|(root, path)| {
                use etas_code::descriptor::ValueProjectionStep as Step;
                if Step::call_inputs(&path).is_empty() {
                    return false;
                }
                // Portable graph projections are resolved by
                // metadata_parameter_origins, which rebases their input IDs.
                // Their original graph-local IDs are not native recipe IDs.
                if matches!(target, ParameterCallTarget::Graph { .. }) {
                    return false;
                }
                let Some(binding) = bindings.get(&target) else {
                    return false;
                };
                if binding.pending_origins() && bindings.contains_key(&root) {
                    return true;
                }
                binding.symbolic_origins()
                    && self.concrete_application_base(
                        target,
                        bindings
                            .iter()
                            .filter_map(|(target, binding)| match binding {
                                TraceArgumentBinding::Origins(sources) => Some((*target, sources)),
                                _ => None,
                            }),
                    )
            });
        if let Some(binding) = bindings.get(&target) {
            // Application inputs can become known after the first capture pass.
            if !reselect {
                return Ok(Some(binding.clone()));
            }
        }
        if let Some((root, path)) = self.parameter_projection_target(target) {
            let element_path = element_origin_path(&path);
            if let Some(binding) = bindings.iter().find_map(|(candidate, binding)| {
                if reselect && (binding.pending_origins() || binding.symbolic_origins()) {
                    return None;
                }
                let (base, selected) = self.parameter_projection_target(*candidate)?;
                (base == root && element_origin_path(&selected) == element_path).then_some(binding)
            }) {
                return Ok(Some(binding.clone()));
            }
            if let ParameterCallTarget::Projection(id) = target
                && let Some(paths) = self
                    .parameter_projection(id)
                    .and_then(|projection| self.value_payload_paths(projection.ty))
                && !paths.is_empty()
            {
                let mut origins = std::collections::BTreeSet::new();
                let mut complete = true;
                for suffix in paths {
                    let mut selected = path.clone();
                    selected.extend(suffix);
                    let selected = element_origin_path(&selected);
                    let child = bindings.iter().find_map(|(candidate, binding)| {
                        if reselect && (binding.pending_origins() || binding.symbolic_origins()) {
                            return None;
                        }
                        let (base, candidate_path) =
                            self.parameter_projection_target(*candidate)?;
                        (base == root && element_origin_path(&candidate_path) == selected)
                            .then_some(binding)
                    });
                    let Some(binding) = child else {
                        complete = false;
                        break;
                    };
                    origins.extend(self.trace_argument_origins(binding, state)?);
                }
                if complete {
                    return Ok(Some(TraceArgumentBinding::Origins(origins)));
                }
            }
            let prefix = bindings
                .iter()
                .filter_map(|(candidate, binding)| {
                    if reselect && (binding.pending_origins() || binding.symbolic_origins()) {
                        return None;
                    }
                    let (base, selected) = self.parameter_projection_target(*candidate)?;
                    if base != root || selected.len() >= path.len() {
                        return None;
                    }
                    let suffix = projected_suffix(&path, &selected)?;
                    Some((selected.len(), suffix, binding))
                })
                .max_by_key(|(length, _, _)| *length);
            if let Some((_, mut suffix, binding)) = prefix {
                self.bind_projection_inputs(&mut suffix, bindings, state)?;
                let suffix = suffix.as_slice();
                return Ok(Some(match binding {
                    TraceArgumentBinding::Expression(expr) => {
                        TraceArgumentBinding::ProjectedExpression {
                            expr: *expr,
                            path: suffix.to_vec(),
                        }
                    }
                    TraceArgumentBinding::Origins(sources) => {
                        TraceArgumentBinding::ProjectedOrigins {
                            sources: sources.clone(),
                            path: suffix.to_vec(),
                        }
                    }
                    TraceArgumentBinding::ProjectedExpression { expr, path: prefix } => {
                        let mut path = prefix.clone();
                        path.extend_from_slice(suffix);
                        TraceArgumentBinding::ProjectedExpression { expr: *expr, path }
                    }
                    TraceArgumentBinding::ProjectedOrigins {
                        sources,
                        path: prefix,
                    } => {
                        let mut path = prefix.clone();
                        path.extend_from_slice(suffix);
                        TraceArgumentBinding::ProjectedOrigins {
                            sources: sources.clone(),
                            path,
                        }
                    }
                    TraceArgumentBinding::AgentTools(ty) => TraceArgumentBinding::AgentTools(*ty),
                    TraceArgumentBinding::InputData {
                        argument,
                        path: prefix,
                        call,
                    } => {
                        let mut path = prefix.clone();
                        path.extend_from_slice(suffix);
                        TraceArgumentBinding::InputData {
                            argument: argument.clone(),
                            path,
                            call: *call,
                        }
                    }
                }));
            }
            if let Some(binding) = bindings.get(&root) {
                let mut path = path;
                self.bind_projection_inputs(&mut path, bindings, state)?;
                let mut binding = binding.clone();
                match &mut binding {
                    TraceArgumentBinding::Expression(expr) => {
                        return Ok(Some(TraceArgumentBinding::ProjectedExpression {
                            expr: *expr,
                            path,
                        }));
                    }
                    TraceArgumentBinding::Origins(sources) => {
                        return Ok(Some(TraceArgumentBinding::ProjectedOrigins {
                            sources: sources.clone(),
                            path,
                        }));
                    }
                    TraceArgumentBinding::ProjectedExpression { path: prefix, .. }
                    | TraceArgumentBinding::ProjectedOrigins { path: prefix, .. }
                    | TraceArgumentBinding::InputData { path: prefix, .. } => {
                        prefix.extend(path);
                        return Ok(Some(binding));
                    }
                    TraceArgumentBinding::AgentTools(_) => {}
                }
            }
            if let ParameterCallTarget::Native(parameter) = root
                && matches!(self.hir.symbols.get(parameter).map(|symbol| &symbol.def), Some(SymbolDef::Param { owner, .. }) if state.owner == Some(*owner))
            {
                return Ok(None);
            }
        }
        if let ParameterCallTarget::AgentTools(context) = target
            && self.agent_context_parameter(context, state.owner).is_some()
        {
            return Ok(None);
        }
        // A caller's checked parameter remains symbolic until that lexical
        // call boundary is instantiated. Foreign unresolved captures are errors.
        if let ParameterCallTarget::Native(symbol) = target
            && self
                .hir
                .symbols
                .get(symbol)
                .is_some_and(|data| matches!(data.def, SymbolDef::Param { owner, .. } if state.owner == Some(owner)))
            && self.checked_callable_contract_parameter(symbol).is_some()
        {
            return Ok(None);
        }
        Err(EffectSpecializationError::MissingLatentEffectBinding {
            param: label.into(),
        })
    }

    pub(crate) fn trace_argument_origins(
        &self,
        binding: &TraceArgumentBinding,
        state: &EffectState,
    ) -> Result<std::collections::BTreeSet<LatentSource>, EffectSpecializationError> {
        match binding {
            TraceArgumentBinding::AgentTools(_) => Ok(Default::default()),
            TraceArgumentBinding::Expression(expr) => {
                if !self.types.facts.expr_types.contains_key(expr) {
                    return Err(EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "call argument has no checked value type".into(),
                    });
                }
                if !self.expr_type_contains_flow(*expr)
                    && !self.expr_type_has_parameters(*expr)
                    && !self.symbolic_contract_expr(state, *expr)
                {
                    return Ok(Default::default());
                }
                Ok(self
                    .latent_sources_for_expr(state, *expr)
                    .unwrap_or_else(|| vec![LatentSource::Unavailable { expr: *expr }])
                    .into_iter()
                    .collect())
            }
            TraceArgumentBinding::Origins(sources) => Ok(sources.clone()),
            TraceArgumentBinding::InputData {
                argument,
                path,
                call,
            } => self
                .input_argument_sources(argument, path, state, *call)
                .ok_or_else(|| EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "callback argument position has no checked origins".into(),
                }),
            TraceArgumentBinding::ProjectedExpression { expr, path } => Ok(self
                .projected_sources(*expr, path, state)
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: *expr }].into())),
            TraceArgumentBinding::ProjectedOrigins { sources, path } => {
                if let [etas_code::descriptor::ValueProjectionStep::Invocation { inputs }] =
                    path.as_slice()
                {
                    return self.applied_callable_origins(sources, *inputs, state);
                }
                let selected = if let Some(inputs) =
                    etas_code::descriptor::ValueProjectionStep::call_inputs(path)
                        .first()
                        .and_then(|id| self.native_call_inputs(*id))
                {
                    self.project_value_origins(sources, path, state, inputs.call)
                } else {
                    self.project_parameter_origins(sources.iter().copied(), path)
                };
                selected.ok_or_else(|| EffectSpecializationError::InvalidCallableEnvironment {
                    reason: format!(
                        "checked argument origins {sources:?} have no selected position {path:?}"
                    ),
                })
            }
        }
    }

    fn trace_argument_summary(
        &self,
        binding: &TraceArgumentBinding,
        state: &EffectState,
        label: &str,
        span: Span,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        match binding {
            TraceArgumentBinding::AgentTools(context) => self
                .agent_tool_summary(*context, span)
                .map_err(|reason| EffectSpecializationError::InvalidCallableEnvironment { reason }),
            TraceArgumentBinding::Expression(expr) => {
                self.latent_summary_for_expr(state, *expr, label)
            }
            TraceArgumentBinding::Origins(sources) => {
                self.latent_summary_for_origins(sources.iter().copied(), span, label)
            }
            TraceArgumentBinding::ProjectedExpression { .. }
            | TraceArgumentBinding::ProjectedOrigins { .. }
            | TraceArgumentBinding::InputData { .. } => self.latent_summary_for_origins(
                self.trace_argument_origins(binding, state)?,
                span,
                label,
            ),
        }
    }

    pub(crate) fn merge_parameter_call_summaries(
        &self,
        target: &mut EffectSummary,
        trace: &ActionTraceDomain,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        inferred_parameters: &BTreeSet<ParameterCallTarget>,
    ) -> Result<(), EffectSpecializationError> {
        let requests =
            self.specialize_handler_requests(&target.handler_requests, bindings, state)?;
        self.merge_parameter_call_summaries_inner(
            target,
            trace,
            bindings,
            state,
            inferred_parameters,
        )?;
        if let Some(behavior) = &mut target.default_behavior {
            behavior.direct.handler_requests = self.specialize_handler_requests(
                &behavior.direct.handler_requests,
                bindings,
                state,
            )?;
            for footprint in behavior.guarded.values_mut() {
                footprint.handler_requests =
                    self.specialize_handler_requests(&footprint.handler_requests, bindings, state)?;
            }
        }
        target.handler_requests = requests;
        Ok(())
    }

    fn specialize_handler_requests(
        &self,
        requests: &crate::RequestInstantiationSummary,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<crate::RequestInstantiationSummary, EffectSpecializationError> {
        let crate::RequestInstantiationSummary::Available {
            actions,
            parameters,
        } = requests
        else {
            return Ok(crate::RequestInstantiationSummary::Unavailable);
        };
        let mut result = crate::RequestInstantiationSummary::Available {
            actions: actions.clone(),
            parameters: BTreeSet::new(),
        };
        for parameter in parameters {
            let Some(binding) =
                self.trace_argument(parameter.target, &parameter.parameter, bindings, state)?
            else {
                if let crate::RequestInstantiationSummary::Available { parameters, .. } =
                    &mut result
                {
                    parameters.insert(parameter.clone());
                }
                continue;
            };
            let mut source = self
                .trace_argument_summary(&binding, state, &parameter.parameter, parameter.span)?
                .handler_requests;
            source.subtract_handled(
                &crate::EffectCoverage {
                    registry: self.registry,
                    types: &self.types.store,
                },
                &EffectRow::closed(parameter.excluded.clone()),
            );
            result.union_assign(&source);
        }
        Ok(result)
    }

    fn merge_parameter_call_summaries_inner(
        &self,
        target: &mut EffectSummary,
        trace: &ActionTraceDomain,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        inferred_parameters: &BTreeSet<ParameterCallTarget>,
    ) -> Result<(), EffectSpecializationError> {
        // This pass unions callback footprints; ordering stays in the
        // independently substituted action trace. Read each cached call site
        // once rather than expanding shared branch and loop graphs.
        for call in trace.parameter_calls() {
            let Some(binding) =
                self.trace_argument(call.target, &call.parameter, bindings, state)?
            else {
                continue;
            };
            let mut source_summary =
                self.trace_argument_summary(&binding, state, &call.parameter, call.span)?;
            source_summary.handler_requests = crate::RequestInstantiationSummary::default();
            self.apply_callback_handlers(target, call.target, call.span, &mut source_summary)?;
            if inferred_parameters.contains(&call.target)
                || matches!(call.target, ParameterCallTarget::AgentTools(_))
            {
                source_summary.action_trace = ActionTraceDomain::Empty;
                target.seq_assign(&source_summary);
            } else {
                target.merge_request_footprint_without_escaping_or_trace(&source_summary);
            }
        }
        Ok(())
    }

    pub(crate) fn inferred_callback_parameters(
        &self,
        parameters: &[ParameterCallTarget],
        types: &[TypeId],
    ) -> Result<BTreeSet<ParameterCallTarget>, EffectSpecializationError> {
        if parameters.len() != types.len() {
            return Err(EffectSpecializationError::InvalidCallableParameterInterface);
        }
        let mut inferred = BTreeSet::new();
        for (parameter, ty) in parameters.iter().zip(types) {
            let ty = self
                .types
                .store
                .get(*ty)
                .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
            if matches!(ty, Type::Function(flow) if flow.declared_effects().is_none()) {
                inferred.insert(*parameter);
            }
        }
        Ok(inferred)
    }

    pub(crate) fn call_trace_bindings(
        &self,
        site: &CallSite<EffectUnit>,
        parameters: &[ParameterCallTarget],
        state: &EffectState,
    ) -> Result<TraceArgumentBindings, EffectSpecializationError> {
        let arguments = self.call_argument_slots(site, parameters.len())?;
        let mut bindings = TraceArgumentBindings::new();
        for (position, (parameter, argument)) in
            parameters.iter().copied().zip(arguments).enumerate()
        {
            let binding = match argument {
                Some(argument) => Some(TraceArgumentBinding::Expression(argument)),
                None => self.default_argument_binding(site.call, position, state)?,
            };
            if let Some(binding) = binding {
                bindings.insert(parameter, binding);
            }
        }
        Ok(bindings)
    }

    pub(crate) fn call_arguments(
        &self,
        site: &CallSite<EffectUnit>,
        arity: usize,
    ) -> Result<Vec<HirExprId>, EffectSpecializationError> {
        self.call_argument_slots(site, arity)?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)
    }

    /// A default in a foreign revision has no expression in the caller's HIR.
    pub(crate) fn call_argument_slots(
        &self,
        site: &CallSite<EffectUnit>,
        arity: usize,
    ) -> Result<Vec<Option<HirExprId>>, EffectSpecializationError> {
        self.argument_slots_at_call(site.call, arity)
    }

    pub(in crate::infer::semantics) fn argument_slots_at_call(
        &self,
        call: HirExprId,
        arity: usize,
    ) -> Result<Vec<Option<HirExprId>>, EffectSpecializationError> {
        if self.types.facts.checked_method_calls.contains_key(&call) {
            let arguments = self.checked_method_argument_slots(call)?;
            if arguments.len() != arity {
                return Err(EffectSpecializationError::InvalidCallableParameterInterface);
            }
            return Ok(arguments);
        }
        let arguments = match self.hir.exprs.get(call) {
            Some(HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. }) => args
                .iter()
                .map(|arg| {
                    arg_expr(arg)
                        .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(HirExpr::SpecMethodCall { receiver, args, .. }) => std::iter::once(Ok(*receiver))
                .chain(args.iter().map(|arg| {
                    arg_expr(arg)
                        .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)
                }))
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(EffectSpecializationError::InvalidCallableParameterInterface),
        };
        if let Some(binding) = self.types.facts.declared_call_arguments.get(&call) {
            let invalid = || EffectSpecializationError::InvalidCallableParameterInterface;
            if arguments.len() != binding.argument_slots.len()
                || arguments.len() + binding.defaults.len() != arity
            {
                return Err(invalid());
            }
            let mut ordered = vec![None; arity];
            for (argument, slot) in arguments.into_iter().zip(&binding.argument_slots) {
                *ordered.get_mut(*slot as usize).ok_or_else(invalid)? = Some(argument);
            }
            for default in &binding.defaults {
                if let etas_types::ParameterDefaultRef::Source { parameter } = default.source {
                    *ordered.get_mut(default.slot as usize).ok_or_else(invalid)? = Some(
                        *self
                            .hir
                            .parameter_defaults
                            .get(&parameter)
                            .ok_or_else(invalid)?,
                    );
                }
            }
            return Ok(ordered);
        }
        if arguments.len() != arity {
            return Err(EffectSpecializationError::InvalidCallableParameterInterface);
        }
        Ok(arguments.into_iter().map(Some).collect())
    }
}
