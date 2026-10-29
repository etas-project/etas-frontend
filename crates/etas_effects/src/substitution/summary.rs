use super::{SummaryMapper, requests::map_requests, trace::map_trace};
use crate::{
    CallableOutcome, EffectSummary, OutcomeAnalysisGap, RequirementFact, RequirementSet,
    TraceSpecClauseFact,
};

pub(crate) fn map_summary<M: SummaryMapper>(
    mapper: &mut M,
    source: &EffectSummary,
) -> Result<EffectSummary, M::Error> {
    mapper.checkpoint()?;
    // Do not clone the recursive trace before mapping it through the worklist.
    let mut result = EffectSummary {
        handler_requests: map_requests(mapper, &source.handler_requests)?,
        default_handler_effects: None,
        exits: None,
        escaping_effects: mapper.row(&source.escaping_effects)?,
        requested_actions: mapper.row(&source.requested_actions)?,
        default_actions: mapper.row(&source.default_actions)?,
        handled_actions: mapper.row(&source.handled_actions)?,
        action_trace: map_trace(mapper, &source.action_trace)?,
        residual_checks: crate::ResidualCheckSet::default(),
        trace_spec_obligations: requirements(mapper, &source.trace_spec_obligations)?,
        requirements: requirements(mapper, &source.requirements)?,
        determinism: source.determinism,
        independent_determinism: source.independent_determinism,
        support: source.support.clone(),
    };
    for check in &source.residual_checks.checks {
        mapper.checkpoint()?;
        result.residual_checks.checks.insert(check.clone());
    }
    if let Some(defaults) = &source.default_handler_effects {
        let mut mapped = crate::DefaultHandlerEffects {
            direct: mapper.row(&defaults.direct)?,
            handlers: Vec::new(),
        };
        for handler in &defaults.handlers {
            mapper.checkpoint()?;
            let action = mapper.effect(handler.action.clone())?;
            if !action.is_action() {
                return Err(mapper.invalid_structure());
            }
            mapped.handlers.push(crate::DefaultHandlerContribution {
                action,
                produced: mapper.row(&handler.produced)?,
            });
        }
        result.default_handler_effects = Some(mapped);
        result.refresh_escaping_effects();
    }
    if let Some(exits) = &source.exits {
        let mut mapped = crate::CallableExitSummary::default();
        for exit in &exits.exits {
            mapper.checkpoint()?;
            let mut outcome = exit.outcome.clone();
            match &mut outcome {
                CallableOutcome::Returned { ty, .. }
                | CallableOutcome::HandlerFinished { ty, .. }
                | CallableOutcome::HandlerResumed { ty, .. } => *ty = mapper.ty(*ty)?,
                CallableOutcome::Raised { error } => *error = mapper.ty(*error)?,
                CallableOutcome::NonReturn => (),
            }
            mapped.exits.push(crate::ExitTraceFact {
                outcome,
                trace: map_trace(mapper, &exit.trace)?,
            });
        }
        for gap in &exits.gaps {
            mapper.checkpoint()?;
            let mut gap = gap.clone();
            if let OutcomeAnalysisGap::ParameterCall { output, .. } = &mut gap {
                *output = mapper.ty(*output)?;
            }
            mapped.gaps.insert(gap);
        }
        for evidence in &exits.handler_gaps {
            mapper.checkpoint()?;
            mapped.handler_gaps.insert(crate::HandlerGapEvidence {
                source: evidence.source.clone(),
                normal_control: evidence.normal_control,
                handled: mapper.row(&evidence.handled)?,
                body_escaping: mapper.row(&evidence.body_escaping)?,
            });
        }
        for exit in &exits.parameter_exits {
            mapper.checkpoint()?;
            mapped.record_parameter_exit(crate::ParameterExitFact {
                target: exit.target,
                output: mapper.ty(exit.output)?,
                source: exit.source.clone(),
                handler_free: exit.handler_free,
                enclosing_handled: exit
                    .enclosing_handled
                    .as_ref()
                    .map(|row| mapper.row(row))
                    .transpose()?,
                arguments: exit
                    .arguments
                    .as_ref()
                    .map(|arguments| {
                        arguments
                            .iter()
                            .map(|argument| {
                                mapper.checkpoint()?;
                                Ok(crate::ParameterExitArgument {
                                    name: argument.name.clone(),
                                    ty: mapper.ty(argument.ty)?,
                                    source: argument.source.clone(),
                                    value: argument.value.clone(),
                                })
                            })
                            .collect()
                    })
                    .transpose()?,
                prefix: map_trace(mapper, &exit.prefix)?,
            });
        }
        result.exits = Some(mapped);
    }
    Ok(result)
}

fn requirements<M: SummaryMapper>(
    mapper: &mut M,
    source: &RequirementSet,
) -> Result<RequirementSet, M::Error> {
    source
        .iter()
        .map(|requirement| {
            mapper.checkpoint()?;
            Ok(match requirement {
                RequirementFact::Limit(limit) => RequirementFact::Limit(limit.clone()),
                RequirementFact::TraceSpec(clause) => RequirementFact::TraceSpec(match clause {
                    TraceSpecClauseFact::Allow { pattern, label } => TraceSpecClauseFact::Allow {
                        pattern: mapper.row(pattern)?,
                        label: label.clone(),
                    },
                    TraceSpecClauseFact::Deny { pattern, label } => TraceSpecClauseFact::Deny {
                        pattern: mapper.row(pattern)?,
                        label: label.clone(),
                    },
                    TraceSpecClauseFact::RequireBefore {
                        guard,
                        guard_label,
                        target,
                        target_label,
                    } => TraceSpecClauseFact::RequireBefore {
                        guard: mapper.row(guard)?,
                        guard_label: guard_label.clone(),
                        target: mapper.row(target)?,
                        target_label: target_label.clone(),
                    },
                    TraceSpecClauseFact::RequireAfter {
                        target,
                        target_label,
                        obligation,
                        obligation_label,
                    } => TraceSpecClauseFact::RequireAfter {
                        target: mapper.row(target)?,
                        target_label: target_label.clone(),
                        obligation: mapper.row(obligation)?,
                        obligation_label: obligation_label.clone(),
                    },
                    other => other.clone(),
                }),
            })
        })
        .collect()
}
