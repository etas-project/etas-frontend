mod agent_effect_legality;
mod fuse_projection_scans;
mod prepare_versioned_request;
mod suffix_demand_history;
mod synthesize_scoped_index;
mod versioned_preparation_cse;

pub use agent_effect_legality::AgentEffectLegalityPass;
pub use fuse_projection_scans::FuseProjectionScansPass;
pub use prepare_versioned_request::PrepareVersionedRequestPass;
pub use suffix_demand_history::SuffixDemandHistoryPass;
pub use synthesize_scoped_index::SynthesizeScopedIndexPass;
pub use versioned_preparation_cse::VersionedPreparationCsePass;
