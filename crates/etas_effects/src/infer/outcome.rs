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
    Handler(HirExprId),
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
}

impl CallableExitSummary {
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
            && !self.exits.is_empty()
            && self
                .exits
                .iter()
                .all(|exit| !matches!(exit.outcome, CallableOutcome::Returned { .. }))
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
        let before = self.gaps.len();
        self.gaps.extend(other.gaps.iter().cloned());
        changed || before != self.gaps.len()
    }

    pub(crate) fn returns_only(&self) -> bool {
        self.gaps.is_empty()
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
    }
}

#[cfg(test)]
mod return_source_tests;
