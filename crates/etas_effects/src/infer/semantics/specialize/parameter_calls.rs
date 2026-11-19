use std::collections::BTreeMap;

use super::EffectSpecializationError;
use crate::{
    ParameterCallTarget,
    infer::semantics::{engine::EffectSemantics, shared::*},
};

#[derive(Clone)]
pub(crate) enum TraceArgumentBinding {
    Expression(HirExprId),
    Origins(std::collections::BTreeSet<LatentSource>),
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

pub(crate) type TraceArgumentBindings = BTreeMap<ParameterCallTarget, TraceArgumentBinding>;

impl EffectSemantics<'_> {
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

    pub(crate) fn trace_argument(
        &self,
        target: ParameterCallTarget,
        label: &str,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<Option<TraceArgumentBinding>, EffectSpecializationError> {
        if let Some(binding) = bindings.get(&target) {
            return Ok(Some(binding.clone()));
        }
        if let Some((root, path)) = self.parameter_projection_target(target) {
            let element_path = element_origin_path(&path);
            if let Some(binding) = bindings.iter().find_map(|(candidate, binding)| {
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
                    let (base, selected) = self.parameter_projection_target(*candidate)?;
                    (base == root && selected.len() < path.len() && path.starts_with(&selected))
                        .then_some((selected, binding))
                })
                .max_by_key(|(selected, _)| selected.len());
            if let Some((prefix, binding)) = prefix {
                let suffix = &path[prefix.len()..];
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
                }));
            }
            if let Some(binding) = bindings.get(&root) {
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
                    | TraceArgumentBinding::ProjectedOrigins { path: prefix, .. } => {
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
            TraceArgumentBinding::ProjectedExpression { expr, path } => Ok(self
                .projected_sources(*expr, path, state)
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: *expr }].into())),
            TraceArgumentBinding::ProjectedOrigins { sources, path } => self
                .project_parameter_origins(sources.iter().copied(), path)
                .ok_or_else(|| EffectSpecializationError::InvalidCallableEnvironment {
                    reason: format!(
                        "checked argument origins {sources:?} have no selected position {path:?}"
                    ),
                }),
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
            | TraceArgumentBinding::ProjectedOrigins { .. } => self.latent_summary_for_origins(
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
        match trace {
            ActionTraceDomain::ParameterCall {
                target: parameter_target,
                parameter,
                span,
            } => {
                let Some(binding) =
                    self.trace_argument(*parameter_target, parameter, bindings, state)?
                else {
                    return Ok(());
                };
                let mut source_summary =
                    self.trace_argument_summary(&binding, state, parameter, *span)?;
                source_summary.handler_requests = crate::RequestInstantiationSummary::default();
                self.apply_callback_handlers(
                    target,
                    *parameter_target,
                    *span,
                    &mut source_summary,
                )?;
                if inferred_parameters.contains(parameter_target)
                    || matches!(parameter_target, ParameterCallTarget::AgentTools(_))
                {
                    source_summary.action_trace = ActionTraceDomain::Empty;
                    target.seq_assign(&source_summary);
                } else {
                    target.merge_request_footprint_without_escaping_or_trace(&source_summary);
                }
            }
            ActionTraceDomain::Seq(items)
            | ActionTraceDomain::Choice(items)
            | ActionTraceDomain::Parallel(items) => {
                for item in items {
                    self.merge_parameter_call_summaries_inner(
                        target,
                        item,
                        bindings,
                        state,
                        inferred_parameters,
                    )?;
                }
            }
            ActionTraceDomain::Repeat(item)
            | ActionTraceDomain::TaskStart { body: item, .. }
            | ActionTraceDomain::DefaultHandler { body: item, .. } => {
                self.merge_parameter_call_summaries_inner(
                    target,
                    item,
                    bindings,
                    state,
                    inferred_parameters,
                )?;
            }
            ActionTraceDomain::Widened {
                parameter_calls, ..
            } => {
                for call in parameter_calls {
                    let Some(binding) =
                        self.trace_argument(call.target, &call.parameter, bindings, state)?
                    else {
                        continue;
                    };
                    let mut source_summary =
                        self.trace_argument_summary(&binding, state, &call.parameter, call.span)?;
                    source_summary.handler_requests = crate::RequestInstantiationSummary::default();
                    self.apply_callback_handlers(
                        target,
                        call.target,
                        call.span,
                        &mut source_summary,
                    )?;
                    if inferred_parameters.contains(&call.target)
                        || matches!(call.target, ParameterCallTarget::AgentTools(_))
                    {
                        source_summary.action_trace = ActionTraceDomain::Empty;
                        target.seq_assign(&source_summary);
                    } else {
                        target.merge_request_footprint_without_escaping_or_trace(&source_summary);
                    }
                }
            }
            ActionTraceDomain::Empty
            | ActionTraceDomain::Event(_)
            | ActionTraceDomain::TaskAwait { .. }
            | ActionTraceDomain::UnknownOrder(_) => {}
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
            if matches!(ty, Type::Function(flow) if flow.effects.is_none()) {
                inferred.insert(*parameter);
            }
        }
        Ok(inferred)
    }

    pub(crate) fn call_trace_bindings(
        &self,
        site: &CallSite<EffectUnit>,
        parameters: &[ParameterCallTarget],
    ) -> Result<TraceArgumentBindings, EffectSpecializationError> {
        let arguments = self.call_argument_slots(site, parameters.len())?;
        Ok(parameters
            .iter()
            .copied()
            .zip(arguments)
            .filter_map(|(parameter, argument)| {
                Some((parameter, TraceArgumentBinding::Expression(argument?)))
            })
            .collect())
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
        if self
            .types
            .facts
            .checked_method_calls
            .contains_key(&site.call)
        {
            let arguments = self.checked_method_argument_slots(site.call)?;
            if arguments.len() != arity {
                return Err(EffectSpecializationError::InvalidCallableParameterInterface);
            }
            return Ok(arguments);
        }
        let arguments = match self.hir.exprs.get(site.call) {
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
        if let Some(binding) = self.types.facts.declared_call_arguments.get(&site.call) {
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

fn element_origin_path(
    path: &[etas_code::descriptor::ValueProjectionStep],
) -> Vec<etas_code::descriptor::ValueProjectionStep> {
    use etas_code::descriptor::ValueProjectionStep as Step;
    // List origins join all elements. Selecting an element after a tail
    // therefore uses the same checked set of possible callable origins.
    path.iter()
        .enumerate()
        .filter_map(|(position, step)| match step {
            Step::ListTail
                if matches!(
                    path.get(position + 1),
                    Some(Step::SequenceElement | Step::ListHead | Step::ListTail)
                ) =>
            {
                None
            }
            Step::ListHead => Some(Step::SequenceElement),
            _ => Some(step.clone()),
        })
        .collect()
}
