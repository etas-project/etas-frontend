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
                self.trace_argument_summary(binding, state, label, span)?
                    .action_trace,
            ))
        })
    }

    fn trace_argument<'b>(
        &self,
        target: ParameterCallTarget,
        label: &str,
        bindings: &'b TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<Option<&'b TraceArgumentBinding>, EffectSpecializationError> {
        if let Some(binding) = bindings.get(&target) {
            return Ok(Some(binding));
        }
        // A caller's checked parameter remains symbolic until that lexical
        // call boundary is instantiated. Foreign unresolved captures are errors.
        if let ParameterCallTarget::Native(symbol) = target
            && self
                .hir
                .symbols
                .get(symbol)
                .is_some_and(|data| matches!(data.def, SymbolDef::Param { owner, .. } if state.owner == Some(owner)))
            && self.checked_function_parameter(symbol).is_some()
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
            TraceArgumentBinding::Expression(expr) => Ok(self
                .latent_sources_for_expr(state, *expr)
                .unwrap_or_else(|| vec![LatentSource::Unavailable { expr: *expr }])
                .into_iter()
                .collect()),
            TraceArgumentBinding::Origins(sources) => Ok(sources.clone()),
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
            TraceArgumentBinding::Expression(expr) => {
                self.latent_summary_for_expr(state, *expr, label)
            }
            TraceArgumentBinding::Origins(sources) => {
                self.latent_summary_for_origins(sources.iter().copied(), span, label)
            }
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
                .trace_argument_summary(binding, state, &parameter.parameter, parameter.span)?
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
                    self.trace_argument_summary(binding, state, parameter, *span)?;
                source_summary.handler_requests = crate::RequestInstantiationSummary::default();
                if inferred_parameters.contains(parameter_target) {
                    source_summary.action_trace = ActionTraceDomain::Empty;
                    target.seq_assign(&source_summary);
                } else {
                    target.merge_request_footprint_without_escaping_or_trace(&source_summary);
                }
            }
            ActionTraceDomain::Seq(items) | ActionTraceDomain::Choice(items) => {
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
            ActionTraceDomain::Repeat(item) => {
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
                        self.trace_argument_summary(binding, state, &call.parameter, call.span)?;
                    source_summary.handler_requests = crate::RequestInstantiationSummary::default();
                    if inferred_parameters.contains(&call.target) {
                        source_summary.action_trace = ActionTraceDomain::Empty;
                        target.seq_assign(&source_summary);
                    } else {
                        target.merge_request_footprint_without_escaping_or_trace(&source_summary);
                    }
                }
            }
            ActionTraceDomain::Empty
            | ActionTraceDomain::Event(_)
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
        let arguments = self.call_arguments(site, parameters.len())?;
        Ok(parameters
            .iter()
            .copied()
            .zip(arguments.into_iter().map(TraceArgumentBinding::Expression))
            .collect())
    }

    pub(crate) fn call_arguments(
        &self,
        site: &CallSite<EffectUnit>,
        arity: usize,
    ) -> Result<Vec<HirExprId>, EffectSpecializationError> {
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
        if arguments.len() != arity {
            return Err(EffectSpecializationError::InvalidCallableParameterInterface);
        }
        Ok(arguments)
    }
}
