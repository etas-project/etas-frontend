use etas_hir::{HirExprId, HirStmtId};
use etas_types::TypeId;

use super::{ActionTraceDomain, EffectUnit};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct FrozenSourceLocation(etas_core::Span);

impl From<etas_core::Span> for FrozenSourceLocation {
    fn from(span: etas_core::Span) -> Self {
        Self(span)
    }
}
impl FrozenSourceLocation {
    pub fn span(&self) -> etas_core::Span {
        self.0
    }
}
impl PartialOrd for FrozenSourceLocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FrozenSourceLocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.0.source.0, self.0.range.start, self.0.range.end).cmp(&(
            other.0.source.0,
            other.0.range.start,
            other.0.range.end,
        ))
    }
}

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
pub enum ReturnObservation {
    Any,
    Unit,
    Bool(bool),
    Variant(etas_hir::SymbolId),
    // Callable values may be returned directly or inside an aggregate payload.
    CallableSources(std::collections::BTreeSet<CallableReturnSource>),
}

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
pub enum CallableReturnSource {
    FrozenContract {
        unit: EffectUnit,
        reference: Option<HirExprId>,
        environment: Option<crate::facts::CallableEnvironmentId>,
    },
    Metadata(MetadataCallableSource),
    Parameter(etas_hir::SymbolId),
    Reference(CallableReturnReference),
    SourceItem(etas_hir::HirItemId),
    AnonymousFlow(EffectUnit),
    Captured {
        unit: EffectUnit,
        environment: crate::facts::CallableEnvironmentId,
    },
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct MetadataCallableSource {
    pub graph: etas_code::CodeFingerprint,
    pub value: u32,
    pub environment: Option<crate::facts::CallableEnvironmentId>,
    pub analysis: CallableAnalysisKind,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum CallableAnalysisKind {
    CheckedRevision,
    FrozenContract,
}

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
pub enum CallableReferenceTarget {
    Source(etas_hir::HirItemId),
    StdIntrinsic(u32),
}

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
pub struct CallableReturnReference {
    pub value: HirExprId,
    pub owner: etas_hir::HirItemId,
    pub environment: Option<crate::facts::CallableEnvironmentId>,
    pub target: CallableReferenceTarget,
    pub signature: TypeId,
    pub type_bindings: Vec<(String, TypeId)>,
    pub effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
}

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
pub enum CallableOutcome {
    Returned {
        ty: TypeId,
        observation: ReturnObservation,
    },
    HandlerFinished {
        ty: TypeId,
        observation: ReturnObservation,
    },
    HandlerResumed {
        ty: TypeId,
        observation: ReturnObservation,
    },
    Raised {
        error: TypeId,
    },
    NonReturn,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExitTraceFact {
    pub outcome: CallableOutcome,
    pub trace: ActionTraceDomain,
}

/// A possible callback termination at its checked call site. `prefix` ends
/// immediately before the callback; its unknown outcome remains a gap until
/// the caller's callback contract can be applied.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ParameterExitFact {
    pub target: crate::ParameterCallTarget,
    pub output: TypeId,
    pub source: FrozenSourceLocation,
    /// None means argument evidence is unavailable, not an empty argument list.
    #[serde(default)]
    pub arguments: Option<Vec<ParameterExitArgument>>,
    /// Whether checked HIR proves that no local handler encloses the call.
    #[serde(default)]
    pub handler_free: Option<bool>,
    /// Checked effects intercepted by enclosing handler scopes; None is unknown.
    #[serde(default)]
    pub enclosing_handled: Option<crate::EffectRow>,
    pub prefix: ActionTraceDomain,
}

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
pub struct HandlerGapEvidence {
    pub source: FrozenSourceLocation,
    pub normal_control: bool,
    pub handled: crate::EffectRow,
    pub body_escaping: crate::EffectRow,
}

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
pub struct ParameterExitArgument {
    pub name: Option<String>,
    pub ty: TypeId,
    pub source: FrozenSourceLocation,
    /// None means the value is not proved by the checked argument expression.
    #[serde(default)]
    pub value: Option<ParameterArgumentValue>,
}

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
pub enum ParameterArgumentValue {
    Bool(bool),
    Integer { negative: bool, magnitude: u128 },
    Input { position: u32 },
}

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
pub enum OutcomeAnalysisGap {
    MissingReturnType,
    MissingReturnObservation,
    MissingPerformedAction(HirExprId),
    InvalidPerformedAction(HirExprId),
    Callee(EffectUnit),
    FrozenCallee(FrozenSourceLocation),
    ExternalCall(HirExprId),
    DynamicCall(HirExprId),
    ParameterCall {
        target: crate::ParameterCallTarget,
        output: TypeId,
        source: FrozenSourceLocation,
    },
    Handler(FrozenSourceLocation),
    Try(HirExprId),
    Operation(HirExprId),
    Loop(HirStmtId),
    EscapingError,
    InvalidControlExit,
}

/// A solved unit's exits, not the footprint of an unfinished expression.
/// Missing data and explicit analysis gaps are never known-empty behavior.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct CallableExitSummary {
    pub exits: Vec<ExitTraceFact>,
    pub gaps: std::collections::BTreeSet<OutcomeAnalysisGap>,
    #[serde(default)]
    pub parameter_exits: Vec<ParameterExitFact>,
    #[serde(default)]
    pub handler_gaps: std::collections::BTreeSet<HandlerGapEvidence>,
}

impl CallableExitSummary {
    pub fn record_parameter_exit(&mut self, exit: ParameterExitFact) -> bool {
        if let Some(existing) = self.parameter_exits.iter_mut().find(|existing| {
            existing.target == exit.target
                && existing.output == exit.output
                && existing.source == exit.source
                && existing.arguments == exit.arguments
        }) {
            let mut changed = existing.prefix.choice_assign(exit.prefix);
            if existing.handler_free != exit.handler_free && existing.handler_free.is_some() {
                existing.handler_free = None;
                changed = true;
            }
            let different_open = matches!(
                (&existing.enclosing_handled, &exit.enclosing_handled),
                (Some(left), Some(right))
                    if left.open.is_some() && right.open.is_some() && left.open != right.open
            );
            if different_open {
                existing.enclosing_handled = None;
                changed = true;
            } else {
                match (&mut existing.enclosing_handled, exit.enclosing_handled) {
                    (Some(existing), Some(incoming)) => changed |= existing.union_assign(&incoming),
                    (existing @ Some(_), None) => {
                        *existing = None;
                        changed = true;
                    }
                    (None, _) => {}
                }
            }
            return changed;
        }
        self.parameter_exits.push(exit);
        self.parameter_exits.sort_by_key(|exit| {
            (
                exit.target,
                exit.output,
                exit.source.clone(),
                exit.arguments.clone(),
            )
        });
        true
    }

    pub(crate) fn handler_finish_sources(
        &self,
    ) -> Option<std::collections::BTreeSet<CallableReturnSource>> {
        if self.gaps.iter().any(|gap| {
            matches!(
                gap,
                OutcomeAnalysisGap::MissingReturnType
                    | OutcomeAnalysisGap::MissingReturnObservation
            )
        }) {
            return None;
        }
        let mut sources = std::collections::BTreeSet::new();
        for exit in &self.exits {
            match &exit.outcome {
                CallableOutcome::HandlerFinished {
                    observation: ReturnObservation::CallableSources(found),
                    ..
                } if !found.is_empty() => sources.extend(found.iter().cloned()),
                CallableOutcome::HandlerFinished { .. } => return None,
                _ => {}
            }
        }
        (!sources.is_empty()).then_some(sources)
    }

    /// Value-origin evidence is independent of whether a preceding call returns,
    /// raises or diverges. Keep all control gaps on this summary; they are not a
    /// certificate of purity or totality. Every successful return still needs a
    /// checked, nonempty source observation.
    pub fn callable_return_sources(
        &self,
    ) -> Option<std::collections::BTreeSet<CallableReturnSource>> {
        if self.gaps.iter().any(|gap| {
            matches!(
                gap,
                OutcomeAnalysisGap::MissingReturnType
                    | OutcomeAnalysisGap::MissingReturnObservation
            )
        }) {
            return None;
        }
        let mut sources = std::collections::BTreeSet::new();
        for exit in &self.exits {
            match &exit.outcome {
                CallableOutcome::Returned {
                    observation: ReturnObservation::CallableSources(found),
                    ..
                } if !found.is_empty() => sources.extend(found.iter().cloned()),
                CallableOutcome::Returned { .. } => return None,
                _ => {}
            }
        }
        (!sources.is_empty()).then_some(sources)
    }
}

impl CallableExitSummary {
    pub(crate) fn has_only_terminal_exits(&self) -> bool {
        self.gaps.is_empty()
            && self.parameter_exits.is_empty()
            && !self.exits.is_empty()
            && self.exits.iter().all(|exit| {
                matches!(
                    exit.outcome,
                    CallableOutcome::Raised { .. } | CallableOutcome::NonReturn
                )
            })
    }

    pub fn record(&mut self, outcome: CallableOutcome, trace: ActionTraceDomain) -> bool {
        if let Some(existing) = self.exits.iter_mut().find(|exit| exit.outcome == outcome) {
            // Empty is epsilon here, not a missing lattice contribution.
            return existing.trace.choice_assign(trace);
        }
        self.exits.push(ExitTraceFact { outcome, trace });
        self.exits
            .sort_by(|left, right| left.outcome.cmp(&right.outcome));
        true
    }

    pub fn join_assign(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for exit in &other.exits {
            changed |= self.record(exit.outcome.clone(), exit.trace.clone());
        }
        for exit in &other.parameter_exits {
            changed |= self.record_parameter_exit(exit.clone());
        }
        let previous = self.handler_gaps.len();
        self.handler_gaps.extend(other.handler_gaps.iter().cloned());
        changed |= previous != self.handler_gaps.len();
        let before = self.gaps.len();
        self.gaps.extend(other.gaps.iter().cloned());
        changed || before != self.gaps.len()
    }

    pub(crate) fn returns_only(&self) -> bool {
        self.gaps.is_empty()
            && self.parameter_exits.is_empty()
            && !self.exits.is_empty()
            && self
                .exits
                .iter()
                .all(|exit| matches!(exit.outcome, CallableOutcome::Returned { .. }))
    }

    pub(crate) fn stabilize(&mut self, previous: Option<&Self>) {
        for exit in &mut self.exits {
            let old = previous.and_then(|previous| {
                previous
                    .exits
                    .iter()
                    .find(|old| old.outcome == exit.outcome)
            });
            exit.trace = exit
                .trace
                .clone()
                .stabilize_for_fixpoint(old.map(|old| &old.trace));
        }
        for exit in &mut self.parameter_exits {
            let old = previous.and_then(|previous| {
                previous.parameter_exits.iter().find(|old| {
                    old.target == exit.target
                        && old.output == exit.output
                        && old.source == exit.source
                        && old.arguments == exit.arguments
                })
            });
            exit.prefix = exit
                .prefix
                .clone()
                .stabilize_for_fixpoint(old.map(|old| &old.prefix));
        }
    }
}

#[cfg(test)]
mod return_source_tests;
