use etas_cache::ProjectRevision;
use etas_utils::PassRunRecord;

use crate::{
    ProjectOutput,
    artifact::DiskCacheAccess,
    incremental::DiagnosticSet,
    session::{ProjectSemanticDelta, ProjectSemanticSnapshot},
};

use super::CacheReuseReport;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckMode {
    #[default]
    FullProject,
    Incremental,
}

impl CheckMode {
    pub fn is_incremental(self) -> bool {
        matches!(self, Self::Incremental)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckScope {
    #[default]
    FullProject,
    EntryReachable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapshotDetailLevel {
    None,
    #[default]
    Full,
}

impl SnapshotDetailLevel {
    pub fn includes_snapshot(self) -> bool {
        matches!(self, Self::Full)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryArtifactReuse {
    Disabled,
    #[default]
    Enabled,
}

impl MemoryArtifactReuse {
    pub const fn disabled() -> Self {
        Self::Disabled
    }

    pub const fn enabled() -> Self {
        Self::Enabled
    }

    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckRequest {
    pub mode: CheckMode,
    pub scope: CheckScope,
    pub disk_artifact_access: DiskCacheAccess,
    pub memory_artifact_reuse: MemoryArtifactReuse,
    pub snapshot_detail: SnapshotDetailLevel,
    pub collect_pipeline_timing: bool,
}

impl CheckRequest {
    pub fn full_project() -> Self {
        Self::default()
    }

    pub fn incremental() -> Self {
        Self {
            mode: CheckMode::Incremental,
            ..Self::default()
        }
    }

    pub fn with_disk_artifact_access(mut self, disk_artifact_access: DiskCacheAccess) -> Self {
        self.disk_artifact_access = disk_artifact_access;
        self
    }

    pub fn with_scope(mut self, scope: CheckScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_memory_artifact_reuse(
        mut self,
        memory_artifact_reuse: MemoryArtifactReuse,
    ) -> Self {
        self.memory_artifact_reuse = memory_artifact_reuse;
        self
    }

    pub fn with_snapshot_detail(mut self, snapshot_detail: SnapshotDetailLevel) -> Self {
        self.snapshot_detail = snapshot_detail;
        self
    }

    pub fn with_pipeline_timing(mut self, enabled: bool) -> Self {
        self.collect_pipeline_timing = enabled;
        self
    }

    pub fn incremental_enabled(&self) -> bool {
        self.mode.is_incremental()
    }

    pub fn memory_artifact_reuse_enabled(&self) -> bool {
        self.memory_artifact_reuse.is_enabled()
    }
}

#[derive(Clone, Debug)]
pub struct CheckResponse {
    pub revision: ProjectRevision,
    pub diagnostics: DiagnosticSet,
    pub snapshot: Option<ProjectSemanticSnapshot>,
    pub delta: Option<ProjectSemanticDelta>,
    pub cache: CacheReuseReport,
    pub output: ProjectOutput,
    pub pipeline_records: Vec<PassRunRecord>,
}
