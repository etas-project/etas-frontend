use super::{CandidateContractAnalysis, ContractComponent, ContractRelation};
use crate::code::definition::FrozenDefinitionContract;
use etas_code::CodeFingerprint;

#[derive(Debug)]
pub(crate) enum ContractArtifactError {
    WrongSnapshot,
    WrongFrozenContract,
    IncompleteComponents,
}

impl std::fmt::Display for ContractArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::WrongSnapshot => "contract result belongs to another snapshot",
            Self::WrongFrozenContract => "contract result belongs to another frozen contract",
            Self::IncompleteComponents => "contract result omits or duplicates a component",
        })
    }
}

impl CandidateContractAnalysis {
    pub(crate) fn validate_for(
        &self,
        snapshot: CodeFingerprint,
        frozen: &FrozenDefinitionContract,
    ) -> Result<bool, ContractArtifactError> {
        match self {
            Self::SourceRejected { snapshot: actual } => {
                if *actual != snapshot {
                    return Err(ContractArtifactError::WrongSnapshot);
                }
                Ok(false)
            }
            Self::Analyzed {
                snapshot: actual,
                frozen_contract,
                checks,
            } => {
                if *actual != snapshot {
                    return Err(ContractArtifactError::WrongSnapshot);
                }
                if *frozen_contract != frozen.fingerprint() {
                    return Err(ContractArtifactError::WrongFrozenContract);
                }
                let count = frozen
                    .callables()
                    .count()
                    .checked_mul(ContractComponent::ALL.len())
                    .ok_or(ContractArtifactError::IncompleteComponents)?;
                if checks.len() != count
                    || frozen.callables().any(|(id, _)| {
                        ContractComponent::ALL.iter().any(|component| {
                            checks
                                .iter()
                                .filter(|check| {
                                    &check.callable == id && check.component == *component
                                })
                                .count()
                                != 1
                        })
                    })
                {
                    return Err(ContractArtifactError::IncompleteComponents);
                }
                Ok(checks
                    .iter()
                    .all(|check| check.relation == ContractRelation::Proven))
            }
        }
    }
}
