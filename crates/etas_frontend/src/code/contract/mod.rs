mod compare;
mod latent;
pub(crate) mod preservation;
mod validation;

use etas_code::{CallableId, CodeFingerprint, contract::TraceInclusionWitness};

pub(crate) use compare::analyze;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractComponent {
    CallableInterface,
    LatentBehavior,
    EscapingEffects,
    RequestedActions,
    Determinism,
    ActionTrace,
    TemporalOutcomes,
    ResidualChecks,
}

impl ContractComponent {
    pub(crate) const ALL: [Self; 8] = [
        Self::CallableInterface,
        Self::LatentBehavior,
        Self::EscapingEffects,
        Self::RequestedActions,
        Self::Determinism,
        Self::ActionTrace,
        Self::TemporalOutcomes,
        Self::ResidualChecks,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractRelation {
    Proven,
    Rejected(ContractRejection),
    Unproven(ContractUncertainty),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractRejection {
    ChangedInterface,
    AdditionalEffects,
    ChangedRequestInstantiation,
    WeakenedDeterminism,
    ActionTraceNotContained(TraceInclusionWitness),
    OutcomeTraceNotContained(TraceInclusionWitness),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractUncertainty {
    MissingFrozenAnalysis,
    MissingCandidateAnalysis,
    MissingOutcomeSensitiveSummary,
    MissingLatentBehaviorSummary,
    UnsupportedResidualChecks,
    TraceProofLimit {
        resource: &'static str,
        limit: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableContractCheck {
    pub callable: CallableId,
    pub component: ContractComponent,
    pub relation: ContractRelation,
}

/// Component results, not an executable candidate or a publication permit.
/// Unproved components must remain rejection obligations for Code.compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateContractAnalysis {
    SourceRejected {
        snapshot: CodeFingerprint,
    },
    Analyzed {
        snapshot: CodeFingerprint,
        frozen_contract: CodeFingerprint,
        checks: Vec<CallableContractCheck>,
    },
}
