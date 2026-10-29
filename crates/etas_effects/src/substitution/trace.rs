use super::SummaryMapper;
use crate::{ActionTraceDomain, EffectSet};

pub(crate) fn map_trace<M: SummaryMapper>(
    mapper: &mut M,
    root: &ActionTraceDomain,
) -> Result<ActionTraceDomain, M::Error> {
    enum Work<'a> {
        Visit(&'a ActionTraceDomain),
        Seq(usize),
        Choice(usize),
        Repeat,
    }
    let mut work = vec![Work::Visit(root)];
    let mut values = Vec::new();
    while let Some(next) = work.pop() {
        mapper.checkpoint()?;
        match next {
            Work::Visit(trace) => match trace {
                ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                    work.push(if matches!(trace, ActionTraceDomain::Seq(_)) {
                        Work::Seq(parts.len())
                    } else {
                        Work::Choice(parts.len())
                    });
                    for part in parts.iter().rev() {
                        mapper.checkpoint()?;
                        work.push(Work::Visit(part));
                    }
                }
                ActionTraceDomain::Repeat(part) => {
                    work.push(Work::Repeat);
                    work.push(Work::Visit(part));
                }
                ActionTraceDomain::Event(event) => {
                    let mut event = event.clone();
                    event.action = mapper.effect(event.action)?;
                    if !event.action.is_action() {
                        return Err(mapper.invalid_structure());
                    }
                    values.push(ActionTraceDomain::Event(event));
                }
                ActionTraceDomain::UnknownOrder(actions) => {
                    values.push(ActionTraceDomain::UnknownOrder(
                        actions
                            .iter()
                            .cloned()
                            .map(|effect| mapper.effect(effect))
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .collect::<EffectSet>(),
                    ))
                }
                ActionTraceDomain::Widened {
                    actions,
                    parameter_calls,
                } => {
                    let mut calls = Vec::new();
                    for parameter in parameter_calls {
                        mapper.checkpoint()?;
                        calls.push(parameter.clone());
                    }
                    values.push(ActionTraceDomain::Widened {
                        actions: actions
                            .iter()
                            .cloned()
                            .map(|effect| mapper.effect(effect))
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .collect(),
                        parameter_calls: calls,
                    });
                }
                ActionTraceDomain::Empty | ActionTraceDomain::ParameterCall { .. } => {
                    values.push(trace.clone())
                }
            },
            Work::Seq(count) | Work::Choice(count) => {
                let start = values
                    .len()
                    .checked_sub(count)
                    .ok_or_else(|| mapper.invalid_structure())?;
                let parts = values.split_off(start);
                values.push(if matches!(next, Work::Seq(_)) {
                    ActionTraceDomain::Seq(parts)
                } else {
                    ActionTraceDomain::Choice(parts)
                });
            }
            Work::Repeat => {
                let value = values.pop().ok_or_else(|| mapper.invalid_structure())?;
                values.push(ActionTraceDomain::Repeat(Box::new(value)));
            }
        }
    }
    if values.len() != 1 {
        return Err(mapper.invalid_structure());
    }
    values.pop().ok_or_else(|| mapper.invalid_structure())
}
