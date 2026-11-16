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
        Parallel(usize),
        TaskStart(etas_hir::HirExprId),
        DefaultHandler(crate::EffectSet),
        Repeat,
    }
    let mut pending = vec![Work::Visit(root)];
    let mut values = Vec::new();
    while let Some(work) = pending.pop() {
        match work {
            Work::Visit(trace) => match trace {
                ActionTraceDomain::DefaultHandler { guards, body } => {
                    pending.push(Work::DefaultHandler(guards.clone()));
                    pending.push(Work::Visit(body));
                }
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
                ActionTraceDomain::Seq(parts)
                | ActionTraceDomain::Choice(parts)
                | ActionTraceDomain::Parallel(parts) => {
                    pending.push(match trace {
                        ActionTraceDomain::Seq(_) => Work::Seq(parts.len()),
                        ActionTraceDomain::Choice(_) => Work::Choice(parts.len()),
                        ActionTraceDomain::Parallel(_) => Work::Parallel(parts.len()),
                        _ => unreachable!("matched compound trace"),
                    });
                    pending.extend(parts.iter().rev().map(Work::Visit));
                }
                ActionTraceDomain::TaskStart { task_site, body } => {
                    pending.push(Work::TaskStart(*task_site));
                    pending.push(Work::Visit(body));
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
                | ActionTraceDomain::TaskAwait { .. }
                | ActionTraceDomain::UnknownOrder(_) => values.push(trace.clone()),
            },
            Work::Seq(count) | Work::Choice(count) | Work::Parallel(count) => {
                let start = values
                    .len()
                    .checked_sub(count)
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                let parts = values.split_off(start);
                if matches!(work, Work::Parallel(_)) {
                    values.push(ActionTraceDomain::Parallel(parts));
                    continue;
                }
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
            Work::TaskStart(task_site) => {
                let body = values
                    .pop()
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                values.push(ActionTraceDomain::TaskStart {
                    task_site,
                    body: Box::new(body),
                });
            }
            Work::DefaultHandler(guards) => {
                let body = values
                    .pop()
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                values.push(ActionTraceDomain::DefaultHandler {
                    guards,
                    body: Box::new(body),
                });
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
