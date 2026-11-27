use super::SummaryMapper;
use crate::{ActionTraceDomain, EffectSet};

pub(crate) fn map_trace<M: SummaryMapper>(
    mapper: &mut M,
    root: &ActionTraceDomain,
) -> Result<ActionTraceDomain, M::Error> {
    map_trace_with_shared(mapper, root, &mut Default::default())
}

// Keys identify live nodes in the source summary. Share this table only
// between roots of that summary under the same substitution.
pub(super) fn map_trace_with_shared<M: SummaryMapper>(
    mapper: &mut M,
    root: &ActionTraceDomain,
    mapped_shared: &mut std::collections::HashMap<usize, ActionTraceDomain>,
) -> Result<ActionTraceDomain, M::Error> {
    enum Work<'a> {
        Visit(&'a ActionTraceDomain),
        Shared(&'a ActionTraceDomain),
        Seq(usize),
        Choice(usize),
        Parallel(usize),
        TaskStart(etas_hir::HirExprId),
        DefaultHandler(EffectSet),
        Repeat,
        Recursive(etas_code::CodeFingerprint),
    }
    let mut work = vec![Work::Visit(root)];
    let mut values = Vec::new();
    while let Some(next) = work.pop() {
        mapper.checkpoint()?;
        match next {
            Work::Visit(trace) => match trace {
                ActionTraceDomain::Shared(body) => {
                    if let Some(mapped) =
                        mapped_shared.get(&(std::sync::Arc::as_ptr(body) as usize))
                    {
                        values.push(mapped.clone());
                        continue;
                    }
                    work.push(Work::Shared(trace));
                    work.push(Work::Visit(&body.body));
                }
                ActionTraceDomain::DefaultHandler { guards, body } => {
                    if let Some(guards) = mapper.default_guards(guards)? {
                        work.push(Work::DefaultHandler(guards));
                        work.push(Work::Visit(body));
                    } else {
                        values.push(ActionTraceDomain::Empty);
                    }
                }
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
                ActionTraceDomain::Recursive { id, body } => {
                    if let Some(body) = body {
                        work.push(Work::Recursive(*id));
                        work.push(Work::Visit(body));
                    } else {
                        values.push(trace.clone());
                    }
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
                        let mut parameter = parameter.clone();
                        parameter.target = mapper.parameter_target(parameter.target)?;
                        calls.push(parameter);
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
                ActionTraceDomain::ParameterCall {
                    target,
                    parameter,
                    span,
                } => values.push(ActionTraceDomain::ParameterCall {
                    target: mapper.parameter_target(*target)?,
                    parameter: parameter.clone(),
                    span: *span,
                }),
                ActionTraceDomain::Empty | ActionTraceDomain::TaskAwait { .. } => {
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
            Work::Shared(original) => {
                let body = values.pop().ok_or_else(|| mapper.invalid_structure())?;
                let ActionTraceDomain::Shared(shared) = original else {
                    unreachable!()
                };
                let mapped = if body == shared.body {
                    original.clone()
                } else {
                    body.shared()
                };
                mapped_shared.insert(std::sync::Arc::as_ptr(shared) as usize, mapped.clone());
                values.push(mapped);
            }
            Work::Recursive(id) => {
                let body = values.pop().ok_or_else(|| mapper.invalid_structure())?;
                values.push(ActionTraceDomain::Recursive {
                    id,
                    body: Some(Box::new(body)),
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
            Work::DefaultHandler(guards) => {
                let body = values.pop().ok_or_else(|| mapper.invalid_structure())?;
                if body == ActionTraceDomain::Empty {
                    values.push(body);
                } else {
                    values.push(ActionTraceDomain::DefaultHandler {
                        guards,
                        body: Box::new(body),
                    });
                }
            }
        }
    }
    if values.len() != 1 {
        return Err(mapper.invalid_structure());
    }
    values.pop().ok_or_else(|| mapper.invalid_structure())
}
