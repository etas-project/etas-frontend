use etas_hir::{HirExprId, HirTypeId};
use etas_utils::ArtifactKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectStage {
    BuildRegistry,
    CollectUnits,
    AnalyzeMemoryProvenance,
    SolveSummaries,
    BindFrozenCalls,
    MaterializeFacts,
    ValidateContracts,
    MaterializeTraceSpecs,
    AnalyzeTraceSpecs,
    LowerResidualPlans,
    VerifyResidualPlans,
    ValidateTraceSpecs,
    Emit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectPipelineError {
    InvalidSourceIdentity {
        source: etas_core::SourceId,
        reason: String,
    },
    InvalidCallableFacts {
        reason: String,
    },
    InvalidTraceSpecSourceSites {
        reason: String,
    },
    AnalysisLimitExceeded {
        resource: &'static str,
        limit: usize,
    },
    AnalysisDidNotConverge {
        stage: EffectStage,
        iterations: usize,
    },
    Interrupted(etas_utils::PipelineInterruption),
    InvalidFrozenSummary {
        reason: String,
    },
    MissingStageArtifact {
        stage: EffectStage,
        artifact: &'static str,
    },
    MissingDiagnosticAnchor {
        artifact: String,
    },
    MissingTypeFact {
        ty: HirTypeId,
    },
    MissingExprFact {
        expr: HirExprId,
    },
    InvalidExternalMetadata {
        package: String,
        reason: String,
    },
    PassManagerFailure {
        reason: String,
        missing_artifact: Option<ArtifactKey>,
    },
    Stopped,
}

impl std::fmt::Display for EffectPipelineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSourceIdentity { source, reason } => write!(
                formatter,
                "invalid effect source identity for {source:?}: {reason}"
            ),
            Self::InvalidCallableFacts { reason } => {
                write!(formatter, "invalid checked callable facts: {reason}")
            }
            Self::InvalidTraceSpecSourceSites { reason } => {
                write!(formatter, "invalid trace spec source sites: {reason}")
            }
            Self::AnalysisLimitExceeded { resource, limit } => write!(
                formatter,
                "effect analysis exceeds {resource} limit {limit}"
            ),
            Self::AnalysisDidNotConverge { stage, iterations } => write!(
                formatter,
                "effect stage {stage:?} did not converge after {iterations} iterations"
            ),
            Self::Interrupted(interruption) => write!(
                formatter,
                "effect analysis interrupted: {}",
                interruption.reason
            ),
            Self::InvalidFrozenSummary { reason } => {
                write!(formatter, "invalid frozen callable summary: {reason}")
            }
            Self::MissingStageArtifact { stage, artifact } => {
                write!(
                    formatter,
                    "effect stage {stage:?} requires artifact `{artifact}`"
                )
            }
            Self::MissingDiagnosticAnchor { artifact } => write!(
                formatter,
                "effect diagnostic for `{artifact}` has no resolved source anchor"
            ),
            Self::MissingTypeFact { ty } => {
                write!(
                    formatter,
                    "effect analysis is missing checked type fact {ty:?}"
                )
            }
            Self::MissingExprFact { expr } => {
                write!(
                    formatter,
                    "effect analysis is missing checked expression fact {expr:?}"
                )
            }
            Self::InvalidExternalMetadata { package, reason } => {
                write!(
                    formatter,
                    "invalid external metadata for `{package}`: {reason}"
                )
            }
            Self::PassManagerFailure {
                reason,
                missing_artifact: Some(artifact),
            } => write!(
                formatter,
                "effect pass manager failed because artifact `{artifact:?}` is missing: {reason}"
            ),
            Self::PassManagerFailure {
                reason,
                missing_artifact: None,
            } => write!(formatter, "effect pass manager failed: {reason}"),
            Self::Stopped => formatter.write_str("effect pipeline stopped before completion"),
        }
    }
}

impl std::error::Error for EffectPipelineError {}
