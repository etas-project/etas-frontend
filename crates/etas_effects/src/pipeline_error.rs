use etas_hir::{HirExprId, HirTypeId};
use etas_utils::ArtifactKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectStage {
    BuildRegistry,
    CollectUnits,
    SolveSummaries,
    MaterializeFacts,
    ValidateContracts,
    MaterializeTraceSpecs,
    AnalyzeTraceSpecs,
    ValidateTraceSpecs,
    Emit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectPipelineError {
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
