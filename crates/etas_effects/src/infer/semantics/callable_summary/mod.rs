mod materialize;
mod prepare;
mod solve;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_core::Span;
use etas_types::TypeId;

use super::{specialize::EffectSpecializationError, state::LatentSource};
use crate::{ActionTraceDomain, EffectSummary, OutcomeAnalysisGap, ParameterCallTarget};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub(crate) struct CallableSummaryKey {
    source: LatentSource,
    mode: CallableSummaryMode,
    source_id: u32,
    start: etas_core::TextSize,
    end: etas_core::TextSize,
}

impl CallableSummaryKey {
    fn in_mode(source: LatentSource, span: Span, mode: CallableSummaryMode) -> Self {
        Self {
            source,
            mode,
            source_id: span.source.0,
            start: span.range.start,
            end: span.range.end,
        }
    }

    fn span(self) -> Span {
        Span::new(
            etas_core::SourceId(self.source_id),
            etas_core::TextRange::new(self.start, self.end),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub(super) enum CallableSummaryMode {
    Invocation,
    Descriptor,
}

#[derive(Default)]
pub(crate) struct CallableSummaryCache {
    summaries: HashMap<CallableSummaryKey, EffectSummary>,
    pub interruption: Option<etas_utils::PipelineInterruption>,
}

impl CallableSummaryCache {
    pub fn invalidate(&mut self) {
        self.summaries.clear();
    }
}

pub(super) enum CallableSummaryPlan {
    Leaf(EffectSummary),
    Dispatch {
        implementations: BTreeSet<CallableSummaryKey>,
        managed: bool,
    },
    Captured(CallableSummaryInstantiation),
}

pub(super) struct CallableSummaryInstantiation {
    pub template: EffectSummary,
    pub bindings: BTreeMap<ParameterCallTarget, BTreeSet<LatentSource>>,
    pub own_parameters: BTreeSet<ParameterCallTarget>,
    pub type_bindings: Vec<(String, TypeId)>,
}

impl CallableSummaryPlan {
    fn dependencies(
        &self,
        mode: CallableSummaryMode,
    ) -> Result<BTreeSet<CallableSummaryKey>, EffectSpecializationError> {
        if let Self::Dispatch {
            implementations, ..
        } = self
        {
            return Ok(implementations.clone());
        }
        let Self::Captured(plan) = self else {
            return Ok(BTreeSet::new());
        };
        let mut dependencies = BTreeSet::new();
        let mut calls = Vec::new();
        let mut pending = vec![&plan.template.action_trace];
        if let Some(exits) = &plan.template.exits {
            pending.extend(exits.exits.iter().map(|exit| &exit.trace));
            pending.extend(exits.parameter_exits.iter().map(|exit| &exit.prefix));
        }
        while let Some(trace) = pending.pop() {
            match trace {
                ActionTraceDomain::Shared(body) => calls.extend(
                    body.parameter_calls
                        .iter()
                        .map(|call| (call.target, call.span)),
                ),
                ActionTraceDomain::ParameterCall { target, span, .. } => {
                    calls.push((*target, *span))
                }
                ActionTraceDomain::Widened {
                    parameter_calls, ..
                } => calls.extend(parameter_calls.iter().map(|call| (call.target, call.span))),
                ActionTraceDomain::Seq(parts)
                | ActionTraceDomain::Choice(parts)
                | ActionTraceDomain::Parallel(parts) => pending.extend(parts),
                ActionTraceDomain::Recursive {
                    body: Some(inner), ..
                }
                | ActionTraceDomain::Repeat(inner)
                | ActionTraceDomain::DefaultHandler { body: inner, .. }
                | ActionTraceDomain::TaskStart { body: inner, .. } => pending.push(inner),
                ActionTraceDomain::Recursive { body: None, .. }
                | ActionTraceDomain::Empty
                | ActionTraceDomain::Event(_)
                | ActionTraceDomain::TaskAwait { .. }
                | ActionTraceDomain::UnknownOrder(_) => {}
            }
        }
        if let Some(exits) = &plan.template.exits {
            calls.extend(exits.gaps.iter().filter_map(|gap| match gap {
                OutcomeAnalysisGap::ParameterCall { target, source, .. } => {
                    Some((*target, source.span()))
                }
                _ => None,
            }));
        }
        for (target, span) in calls {
            match plan.bindings.get(&target) {
                Some(origins) if !origins.is_empty() => dependencies.extend(
                    origins
                        .iter()
                        .map(|source| CallableSummaryKey::in_mode(*source, span, mode)),
                ),
                Some(origins)
                    if origins.is_empty()
                        && matches!(
                            target,
                            ParameterCallTarget::Projection(_)
                                | ParameterCallTarget::ExternalProjection(_)
                                | ParameterCallTarget::Graph {
                                    parameter: etas_package_metadata::CallableValueParameter {
                                        projection: Some(_),
                                        ..
                                    },
                                    ..
                                }
                        ) => {}
                None if plan.own_parameters.contains(&target) => {}
                _ => {
                    return Err(invalid(&format!(
                        "captured callable has no checked free-parameter binding for {target:?}",
                    )));
                }
            }
        }
        Ok(dependencies)
    }
}

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}
