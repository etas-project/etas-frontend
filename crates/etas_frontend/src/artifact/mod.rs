mod body_index;
mod dependency;
mod disk_store;
mod fingerprint;
mod key;
mod kind;
mod manifest;
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
pub use fingerprint::fingerprint_text;
pub use key::{FrontendArtifactKey, FrontendUnitKey};
pub use kind::{FRONTEND_ARTIFACT_SCHEMA_VERSION, FRONTEND_CACHE_NAMESPACE, FrontendArtifactKind};
pub(crate) use manifest::PersistedFrontendArtifactManifest;
pub use manifest::{FrontendArtifactManifest, FrontendArtifactRecord};
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
