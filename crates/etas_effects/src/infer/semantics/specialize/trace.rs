use super::EffectSpecializationError;
use crate::{ActionTraceDomain, ParameterCallTarget};
use etas_core::Span;

pub(crate) enum ParameterCallSubstitution {
    Symbolic,
    Trace(ActionTraceDomain),
}

pub(crate) fn substitute_parameter_calls(
    root: &ActionTraceDomain,
    mut resolve: impl FnMut(
        ParameterCallTarget,
        &str,
        Span,
    ) -> Result<ParameterCallSubstitution, EffectSpecializationError>,
) -> Result<ActionTraceDomain, EffectSpecializationError> {
    enum Work<'a> {
        Visit(&'a ActionTraceDomain),
        Seq(usize),
        Choice(usize),
        Repeat,
    }
    let mut pending = vec![Work::Visit(root)];
    let mut values = Vec::new();
    while let Some(work) = pending.pop() {
        match work {
            Work::Visit(trace) => match trace {
                ActionTraceDomain::ParameterCall {
                    target,
                    parameter,
                    span,
                } => {
                    values.push(match resolve(*target, parameter, *span)? {
                        ParameterCallSubstitution::Symbolic => trace.clone(),
                        ParameterCallSubstitution::Trace(trace) => trace,
                    });
                }
                ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                    pending.push(if matches!(trace, ActionTraceDomain::Seq(_)) {
                        Work::Seq(parts.len())
                    } else {
                        Work::Choice(parts.len())
                    });
                    pending.extend(parts.iter().rev().map(Work::Visit));
                }
                ActionTraceDomain::Repeat(part) => {
                    pending.push(Work::Repeat);
                    pending.push(Work::Visit(part));
                }
                ActionTraceDomain::Widened {
                    actions,
                    parameter_calls,
                } => {
                    let mut result = ActionTraceDomain::UnknownOrder(actions.clone());
                    for call in parameter_calls {
                        result.seq_assign(
                            match resolve(call.target, &call.parameter, call.span)? {
                                ParameterCallSubstitution::Trace(trace) => trace,
                                ParameterCallSubstitution::Symbolic => {
                                    ActionTraceDomain::ParameterCall {
                                        target: call.target,
                                        parameter: call.parameter.clone(),
                                        span: call.span,
                                    }
                                }
                            },
                        );
                    }
                    values.push(result.widened());
                }
                ActionTraceDomain::Empty
                | ActionTraceDomain::Event(_)
                | ActionTraceDomain::UnknownOrder(_) => values.push(trace.clone()),
            },
            Work::Seq(count) | Work::Choice(count) => {
                let start = values
                    .len()
                    .checked_sub(count)
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                let parts = values.split_off(start);
                let mut parts = parts.into_iter();
                let mut trace = parts.next().unwrap_or_default();
                for part in parts {
                    if matches!(work, Work::Seq(_)) {
                        trace.seq_assign(part);
                    } else {
                        trace.choice_assign(part);
                    }
                }
                values.push(trace);
            }
            Work::Repeat => {
                let value = values
                    .pop()
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                values.push(value.repeat());
            }
        }
    }
    if values.len() != 1 {
        return Err(EffectSpecializationError::InvalidCallableReturnSources);
    }
    values
        .pop()
        .ok_or(EffectSpecializationError::InvalidCallableReturnSources)
}
