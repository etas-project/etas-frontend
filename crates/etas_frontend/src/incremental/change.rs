use etas_cache::{ArtifactKey, ProjectRevision};
use etas_core::SourceId;

use crate::{ExternalPackageId, ProjectEnvironmentInput, SourceInput};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceVersion(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

#[derive(Clone, Debug)]
pub struct ProjectChangeSet {
    pub revision: ProjectRevision,
    pub source_changes: Vec<SourceChange>,
    pub dependency_overlay_changes: Vec<DependencyOverlayChange>,
    pub environment_change: Option<EnvironmentChange>,
    pub option_changes: Vec<CompilerOptionChange>,
}

#[derive(Clone, Debug)]
pub struct DependencyOverlayChange {
    pub package: ExternalPackageId,
    pub import_root: String,
    pub added_sources: Vec<SourceInput>,
}

#[derive(Clone, Debug)]
pub enum SourceChange {
    Add(SourceInput),
    Remove(SourceId),
    Replace {
        source: SourceId,
        version: SourceVersion,
        text: String,
    },
    Edit {
        source: SourceId,
        version: SourceVersion,
        edits: Vec<TextEdit>,
    },
}

#[derive(Clone, Debug)]
pub enum EnvironmentChange {
    Replace(ProjectEnvironmentInput),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerOptionChange {
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSummary {
    pub revision: ProjectRevision,
    pub changed_sources: Vec<SourceId>,
    pub added_sources: Vec<SourceId>,
    pub removed_sources: Vec<SourceId>,
    pub invalidated_artifacts: Vec<ArtifactKey>,
}
