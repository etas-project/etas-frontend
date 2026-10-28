mod descriptors;
mod emit;
pub(crate) use descriptors::{
    BuildDefinitionDescriptorsPass, CANDIDATE_DESCRIPTORS, CandidateDescriptors,
};
mod invocation_types;
mod overlay;
mod reflection_targets;
mod sources;
mod target;
mod validate_contract;
pub(crate) use emit::{CANDIDATE_EMISSION, EmitCheckedCandidatePass};
pub(crate) use invocation_types::{
    CANDIDATE_INVOCATION_TYPES, MaterializeCandidateInvocationTypesPass,
};
pub(crate) use overlay::ApplyCandidateBodyOverlaysPass;
pub(crate) use reflection_targets::ResolveReflectionTargetsPass;
pub(crate) use sources::FreezeDefinitionEnvironmentPass;
pub(crate) use sources::ParseCandidateDependencySourcesPass;
pub(crate) use target::ResolveCandidateTargetPass;
pub(crate) use validate_contract::{CANDIDATE_CONTRACT_ANALYSIS, ValidateCandidateContractPass};
