use std::collections::BTreeSet;

use crate::{Effect, EffectCoverage, EffectRow, EffectSet, ParameterCallTarget};
use etas_types::TypeId;

/// Request ABI evidence, separate from authority selectors and the audit trace.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ActionRequestInstance {
    pub action: Effect,
    pub type_bindings: Option<Vec<(String, TypeId)>>,
}

#[cfg(test)]
mod tests;

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ParameterRequest {
    pub target: ParameterCallTarget,
    pub parameter: String,
    pub span: etas_core::Span,
    /// Lexically installed total handlers, not default-provider fallbacks.
    pub excluded: EffectSet,
}

impl PartialOrd for ParameterRequest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParameterRequest {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            self.target,
            &self.parameter,
            &self.excluded,
            self.span.source.0,
            self.span.range.start.0,
            self.span.range.end.0,
        )
            .cmp(&(
                other.target,
                &other.parameter,
                &other.excluded,
                other.span.source.0,
                other.span.range.start.0,
                other.span.range.end.0,
            ))
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum RequestInstantiationSummary {
    Available {
        actions: BTreeSet<ActionRequestInstance>,
        parameters: BTreeSet<ParameterRequest>,
    },
    Unavailable,
}

impl Default for RequestInstantiationSummary {
    fn default() -> Self {
        Self::Available {
            actions: BTreeSet::new(),
            parameters: BTreeSet::new(),
        }
    }
}

impl RequestInstantiationSummary {
    pub fn less_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (_, Self::Unavailable) => true,
            (Self::Unavailable, _) => false,
            (
                Self::Available {
                    actions,
                    parameters,
                },
                Self::Available {
                    actions: rhs,
                    parameters: rparams,
                },
            ) => actions.is_subset(rhs) && parameters.is_subset(rparams),
        }
    }
    pub fn action(action: Effect, type_bindings: Vec<(String, TypeId)>) -> Self {
        Self::Available {
            actions: BTreeSet::from([ActionRequestInstance {
                action,
                type_bindings: Some(type_bindings),
            }]),
            parameters: BTreeSet::new(),
        }
    }

    pub fn record_untyped(&mut self, action: Effect) {
        if let Self::Available { actions, .. } = self {
            actions.insert(ActionRequestInstance {
                action,
                type_bindings: None,
            });
        }
    }

    pub fn parameter(
        target: ParameterCallTarget,
        parameter: String,
        span: etas_core::Span,
    ) -> Self {
        Self::Available {
            actions: BTreeSet::new(),
            parameters: BTreeSet::from([ParameterRequest {
                target,
                parameter,
                span,
                excluded: EffectSet::new(),
            }]),
        }
    }

    pub fn union_assign(&mut self, other: &Self) -> bool {
        match (&mut *self, other) {
            (
                Self::Available {
                    actions,
                    parameters,
                },
                Self::Available {
                    actions: rhs,
                    parameters: rparams,
                },
            ) => {
                let before = (actions.len(), parameters.len());
                actions.extend(rhs.iter().cloned());
                parameters.extend(rparams.iter().cloned());
                before != (actions.len(), parameters.len())
            }
            (Self::Unavailable, _) => false,
            (_, Self::Unavailable) => {
                *self = Self::Unavailable;
                true
            }
        }
    }

    pub fn subtract_handled(&mut self, coverage: &EffectCoverage<'_>, handled: &EffectRow) {
        if let Self::Available {
            actions,
            parameters,
        } = self
        {
            actions.retain(|instance| {
                !handled
                    .effects
                    .iter()
                    .any(|handler| coverage.covers(handler, &instance.action))
            });
            *parameters = std::mem::take(parameters)
                .into_iter()
                .map(|mut parameter| {
                    parameter.excluded.union_assign(&handled.effects);
                    parameter
                })
                .collect();
        }
    }
}
