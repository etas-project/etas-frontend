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
        Parallel(usize),
        TaskStart(etas_hir::HirExprId),
        Repeat,
    }
    let mut work = vec![Work::Visit(root)];
    let mut values = Vec::new();
    while let Some(next) = work.pop() {
        mapper.checkpoint()?;
        match next {
            Work::Visit(trace) => match trace {
                ActionTraceDomain::Seq(parts)
                | ActionTraceDomain::Choice(parts)
                | ActionTraceDomain::Parallel(parts) => {
                    work.push(match trace {
                        ActionTraceDomain::Seq(_) => Work::Seq(parts.len()),
                        ActionTraceDomain::Choice(_) => Work::Choice(parts.len()),
                        ActionTraceDomain::Parallel(_) => Work::Parallel(parts.len()),
                        _ => unreachable!("matched compound trace"),
                    });
                    for part in parts.iter().rev() {
                        mapper.checkpoint()?;
                        work.push(Work::Visit(part));
                    }
                }
                ActionTraceDomain::TaskStart { task_site, body } => {
                    work.push(Work::TaskStart(*task_site));
                    work.push(Work::Visit(body));
                }
                ActionTraceDomain::Repeat(part) => {
                    work.push(Work::Repeat);
                    work.push(Work::Visit(part));
                }
                ActionTraceDomain::Event(event) => {
                    values.push(mapper.event(event)?);
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
                ActionTraceDomain::Empty
                | ActionTraceDomain::ParameterCall { .. }
                | ActionTraceDomain::TaskAwait { .. } => {
                    values.push(trace.clone())
                }
            },
            Work::Seq(count) | Work::Choice(count) | Work::Parallel(count) => {
                let start = values
                    .len()
                    .checked_sub(count)
                    .ok_or_else(|| mapper.invalid_structure())?;
                let parts = values.split_off(start);
                values.push(match next {
                    Work::Seq(_) => ActionTraceDomain::Seq(parts),
                    Work::Choice(_) => ActionTraceDomain::Choice(parts),
                    Work::Parallel(_) => ActionTraceDomain::Parallel(parts),
                    _ => unreachable!("matched compound trace"),
                });
            }
            Work::Repeat => {
                let value = values.pop().ok_or_else(|| mapper.invalid_structure())?;
                values.push(ActionTraceDomain::Repeat(Box::new(value)));
            }
            Work::TaskStart(task_site) => {
                let body = values.pop().ok_or_else(|| mapper.invalid_structure())?;
                values.push(ActionTraceDomain::TaskStart {
                    task_site,
                    body: Box::new(body),
                });
            }
        }
    }
    if values.len() != 1 {
        return Err(mapper.invalid_structure());
    }
    values.pop().ok_or_else(|| mapper.invalid_structure())
}
