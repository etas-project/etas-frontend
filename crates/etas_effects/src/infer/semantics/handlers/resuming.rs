use super::*;
use crate::substitution::{SummaryMapper, map_trace};
use crate::{ActionEvent, ActionEventSource, ActionTraceDomain, Effect, OutcomeAnalysisGap};

struct ArmTrace {
    effect: Effect,
    trace: ActionTraceDomain,
}

struct RequestTrace {
    call: HirExprId,
    action: Effect,
    span: Span,
    trace: ActionTraceDomain,
}

enum TraceExpansionError {
    Unproved,
    Interrupted(etas_utils::PipelineInterruption),
}

struct HandlerTraceMapper<'a> {
    requests: &'a [RequestTrace],
    handled: &'a EffectRow,
    coverage: EffectCoverage<'a>,
    has_arm_actions: bool,
    hits: Vec<usize>,
    remaining: usize,
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

impl SummaryMapper for HandlerTraceMapper<'_> {
    type Error = TraceExpansionError;

    fn checkpoint(&mut self) -> Result<(), Self::Error> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(TraceExpansionError::Unproved)?;
        if let Some(control) = self.control {
            control
                .checkpoint()
                .map_err(TraceExpansionError::Interrupted)?;
        }
        Ok(())
    }

    fn invalid_structure(&self) -> Self::Error {
        TraceExpansionError::Unproved
    }
    fn unsupported_residual_substitution(&self) -> Self::Error {
        TraceExpansionError::Unproved
    }
    fn ty(&mut self, ty: TypeId) -> Result<TypeId, Self::Error> {
        Ok(ty)
    }
    fn effect(&mut self, effect: Effect) -> Result<Effect, Self::Error> {
        Ok(effect)
    }
    fn row(&mut self, row: &EffectRow) -> Result<EffectRow, Self::Error> {
        Ok(row.clone())
    }

    fn event(&mut self, event: &ActionEvent) -> Result<ActionTraceDomain, Self::Error> {
        if !event.action.is_action() {
            return Err(TraceExpansionError::Unproved);
        }
        let mut result = ActionTraceDomain::Event(event.clone());
        let mut matches = self.requests.iter().enumerate().filter(|(_, request)| {
            event.source == ActionEventSource::Perform
                && request.span == event.span
                && request.action == event.action
        });
        let Some((index, request)) = matches.next() else {
            if self.has_arm_actions
                && self
                    .handled
                    .effects
                    .iter()
                    .any(|effect| self.coverage.covers(effect, &event.action))
            {
                return Err(TraceExpansionError::Unproved);
            }
            return Ok(result);
        };
        if matches.next().is_some() {
            return Err(TraceExpansionError::Unproved);
        }
        let mut work = vec![&request.trace];
        while let Some(trace) = work.pop() {
            self.checkpoint()?;
            match trace {
                ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                    work.extend(parts)
                }
                ActionTraceDomain::Repeat(inner) => work.push(inner),
                _ => {}
            }
        }
        result.seq_assign(request.trace.clone());
        self.hits[index] += 1;
        Ok(result)
    }
}

fn action_ref(effect: &Effect) -> Option<&crate::ActionRef> {
    match effect {
        Effect::Action(action) => Some(action),
        Effect::AppliedAction(action) => Some(&action.action),
        _ => None,
    }
}

impl EffectSemantics<'_> {
    /// Handler bodies are analyzed in isolation. Prove total resumption from
    /// solved exits, then splice each arm trace at its owned request, not at
    /// the end of the handled block or once per action identity.
    pub(super) fn checked_resuming_handler_completion(
        &self,
        handler: HirExprId,
        body: &mut EffectState,
        (handled, produced): (&EffectRow, &EffectRow),
    ) -> Result<bool, etas_utils::PipelineInterruption> {
        if body.incomplete
            || !produced.is_empty()
            || handled.open.is_some()
            || !body.exits.parameter_exits.is_empty()
            || !body.exits.handler_gaps.is_empty()
        {
            return Ok(false);
        }
        let Some((_, fact)) = self.immutable_handler_value(handler)? else {
            return Ok(false);
        };
        if fact.arms.is_empty() {
            return Ok(false);
        }
        let mut arms = Vec::new();
        for arm in &fact.arms {
            if let Some(control) = self.query_control {
                control.checkpoint()?;
            }
            let EffectUnit::HandlerArm { arm: id, .. } = arm.unit else {
                return Ok(false);
            };
            let (Some(summary), Some(checked), Some(effect)) = (
                self.inputs.unit_effects.get(&arm.unit),
                self.types.facts.handler_actions.get(&id),
                self.inputs.handler_action_effects.get(&id),
            ) else {
                return Ok(false);
            };
            let Some(exits) = &summary.exits else {
                return Ok(false);
            };
            let total_resumption = !exits.exits.is_empty()
                && exits.exits.iter().all(|exit| {
                    matches!(exit.outcome, crate::CallableOutcome::HandlerResumed { ty, .. } if ty == checked.output)
                });
            if !arm.resumable
                || matches!(summary.support, InterpreterSupport::Rejected(_))
                || !summary.escaping_effects.is_empty()
                || !exits.gaps.is_empty()
                || !exits.parameter_exits.is_empty()
                || !exits.handler_gaps.is_empty()
                || !total_resumption
            {
                return Ok(false);
            }
            let mut trace: Option<ActionTraceDomain> = None;
            for exit in &exits.exits {
                match &mut trace {
                    Some(trace) => {
                        trace.choice_assign(exit.trace.clone());
                    }
                    trace @ None => *trace = Some(exit.trace.clone()),
                }
            }
            let Some(trace) = trace else {
                return Ok(false);
            };
            arms.push(ArmTrace {
                effect: effect.clone(),
                trace,
            });
        }
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let mut requests = Vec::new();
        for gap in &body.exits.gaps {
            let OutcomeAnalysisGap::ExternalCall(call) = gap else {
                return Ok(false);
            };
            if !matches!(self.hir.exprs.get(*call), Some(HirExpr::Perform { .. })) {
                return Ok(false);
            }
            let Some(action) = self.inputs.performed_actions.get(call) else {
                return Ok(false);
            };
            if action.expr != *call
                || !handled
                    .effects
                    .iter()
                    .any(|effect| coverage.covers(effect, &action.invocation))
            {
                return Ok(false);
            }
            let scope = super::super::argument_values::nearest_enclosing_handler(
                self.hir,
                *call,
                self.query_control,
            )?;
            if scope != Some(handler) {
                return Ok(false);
            }
            let Some(target) = action_ref(&action.invocation) else {
                return Ok(false);
            };
            let mut trace: Option<ActionTraceDomain> = None;
            for arm in arms
                .iter()
                .filter(|arm| action_ref(&arm.effect) == Some(target))
            {
                match &mut trace {
                    Some(trace) => {
                        trace.choice_assign(arm.trace.clone());
                    }
                    trace @ None => *trace = Some(arm.trace.clone()),
                }
            }
            let Some(trace) = trace else {
                return Ok(false);
            };
            requests.push(RequestTrace {
                call: *call,
                action: action.invocation.clone(),
                span: action.span,
                trace,
            });
        }
        if body.summary.action_trace.has_unordered_approximation()
            || body
                .exits
                .exits
                .iter()
                .any(|exit| exit.trace.has_unordered_approximation())
        {
            return Ok(false);
        }
        let mut mapper = HandlerTraceMapper {
            requests: &requests,
            handled,
            coverage,
            has_arm_actions: arms.iter().any(|arm| arm.trace != ActionTraceDomain::Empty),
            hits: vec![0; requests.len()],
            remaining: etas_code::descriptor::DescriptorLimits::default().max_nodes,
            control: self.query_control,
        };
        let mapped: Result<_, TraceExpansionError> = (|| {
            let trace = map_trace(&mut mapper, &body.summary.action_trace)?;
            if mapper.hits.contains(&0) {
                return Err(TraceExpansionError::Unproved);
            }
            let exits = body
                .exits
                .exits
                .iter()
                .map(|exit| map_trace(&mut mapper, &exit.trace))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((trace, exits))
        })();
        match mapped {
            Ok((trace, exits)) => {
                body.summary.action_trace = trace;
                for (exit, trace) in body.exits.exits.iter_mut().zip(exits) {
                    exit.trace = trace;
                }
                for request in &requests {
                    body.exits
                        .gaps
                        .remove(&OutcomeAnalysisGap::ExternalCall(request.call));
                }
                Ok(true)
            }
            Err(TraceExpansionError::Unproved) => Ok(false),
            Err(TraceExpansionError::Interrupted(interruption)) => Err(interruption),
        }
    }
}
