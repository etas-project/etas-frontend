mod body_index;
mod dependency;
mod disk_store;
mod external_fingerprint;
mod fingerprint;
mod key;
mod kind;
mod manifest;
mod manifest_input;
mod metadata;
mod policy;
mod signature_facts;
mod summary;

pub(crate) use body_index::{
    BodyArtifactIdentityRecord, BodyArtifactReuseIndex, BodyArtifactReuseIndexRecord,
    BodyArtifactReuseKind, PersistedBodyArtifactReuseIndex, PersistedFrontendArtifactDependency,
    PersistedFrontendArtifactKey,
};
pub use dependency::{FrontendArtifactDependency, cache_dependencies};
pub use disk_store::FrontendDiskArtifactStore;
pub(crate) use external_fingerprint::{
    dependency_source_package, external_action, external_action_summary, external_agent,
    external_callable_satisfaction, external_dependency_effects, external_effect_summary,
    external_flow, external_module, external_named_signature, external_package, external_re_export,
    external_spec, external_spec_impl, external_tool, external_tool_binding, external_tool_schema,
    external_trace_conformance, external_trace_summary, external_type_satisfaction,
};
pub use fingerprint::fingerprint_text;
pub use key::{FrontendArtifactKey, FrontendUnitKey};
pub use kind::{FRONTEND_ARTIFACT_SCHEMA_VERSION, FRONTEND_CACHE_NAMESPACE, FrontendArtifactKind};
pub(crate) use manifest::PersistedFrontendArtifactManifest;
pub use manifest::{FrontendArtifactManifest, FrontendArtifactRecord};
pub(crate) use manifest_input::ArtifactManifestInput;
pub use metadata::FRONTEND_COMPILER_VERSION;
pub(crate) use metadata::{
    changed_compiler_options_hash, frontend_artifact_meta, frontend_std_version,
};
pub use policy::{DiskCacheAccess, PersistenceClass, persistence_class};
pub(crate) use signature_facts::PersistedItemSignature;
pub use summary::{
    ArtifactDependencySummary, ArtifactReuseStatsSummary, ModuleImportExportSummary,
    SourceFingerprintSummary,
};
