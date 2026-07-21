mod affected;
mod body_reuse;
mod change;
mod diagnostics;
mod dirty;
mod invalidation;
mod request;
mod reuse;

pub(crate) use affected::{affected_bodies, affected_items, affected_modules};
pub(crate) use body_reuse::{
    BodyArtifactIdentity, BodyArtifactReuseInput, CachedEffectBodyArtifact, CachedTypeBodyArtifact,
    dependency_fingerprints_match,
};
pub use change::{
    ChangeSummary, CompilerOptionChange, DependencyOverlayChange, EnvironmentChange,
    ProjectChangeSet, SourceChange, SourceVersion, TextEdit,
};
pub use diagnostics::DiagnosticSet;
pub(crate) use dirty::DirtyArtifactRoots;
pub(crate) use invalidation::invalidate_dirty_roots;
pub use request::{
    CheckMode, CheckRequest, CheckResponse, CheckScope, MemoryArtifactReuse, SnapshotDetailLevel,
};
pub use reuse::CacheReuseReport;
pub(crate) use reuse::artifact_meta_matches;
