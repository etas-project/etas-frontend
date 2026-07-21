use std::{collections::HashMap, path::PathBuf};

use etas_cache::{CacheError, MemoryArtifactStore, ProjectRevision};
use etas_core::{SourceId, id_type};

use crate::artifact::FrontendArtifactManifest;
use crate::incremental::{
    ChangeSummary, CheckRequest, CheckResponse, ProjectChangeSet, SourceVersion,
};
use crate::{ExternalPackageId, ProjectInput, SourceSet};

mod change;
mod delta;
mod options;
mod project;
mod snapshot;
mod source_state;
mod store;

use project::FrontendProjectState;

pub use delta::{DiagnosticDelta, ProjectSemanticDelta};
pub use options::{FrontendCacheConfig, FrontendSessionOptions};
pub use snapshot::{DefinitionTarget, ProjectSemanticSnapshot, SnapshotDefinitionTargets};
pub use store::{FrontendArtifactStore, FrontendSessionStore};

id_type!(ProjectSessionId);

#[derive(Debug, PartialEq, Eq)]
pub enum FrontendSessionError {
    UnknownProject(ProjectSessionId),
    NonIncreasingProjectRevision {
        current: ProjectRevision,
        requested: ProjectRevision,
    },
    NonIncreasingSourceVersion {
        source: SourceId,
        current: SourceVersion,
        requested: SourceVersion,
    },
    MissingSource(SourceId),
    DuplicateSource(SourceId),
    DuplicateSourcePath(PathBuf),
    EmptyDependencyOverlay {
        package: ExternalPackageId,
        import_root: String,
    },
    InvalidDependencyOverlaySource {
        source: SourceId,
    },
    MismatchedDependencyOverlaySource {
        source: SourceId,
        expected_package: ExternalPackageId,
        expected_import_root: String,
    },
    InvalidTextEdit {
        source: SourceId,
        start: usize,
        end: usize,
        len: usize,
    },
    Pipeline(String),
    Cache(String),
}

pub struct FrontendSession<S = MemoryArtifactStore>
where
    S: FrontendArtifactStore,
{
    next_project: u32,
    projects: HashMap<ProjectSessionId, FrontendProjectState<S>>,
    new_store: Box<dyn Fn() -> Result<S, FrontendSessionError>>,
}

impl Default for FrontendSession<MemoryArtifactStore> {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontendSession<MemoryArtifactStore> {
    pub fn new() -> Self {
        Self::with_store_factory(MemoryArtifactStore::new)
    }
}

impl FrontendSession<FrontendSessionStore> {
    pub fn with_options(options: FrontendSessionOptions) -> Result<Self, FrontendSessionError> {
        FrontendSessionStore::from_options(&options).map_err(cache_error)?;
        Ok(Self {
            next_project: 0,
            projects: HashMap::new(),
            new_store: Box::new(move || {
                FrontendSessionStore::from_options(&options).map_err(cache_error)
            }),
        })
    }
}

impl<S> FrontendSession<S>
where
    S: FrontendArtifactStore + 'static,
{
    pub fn with_store_factory(new_store: fn() -> S) -> Self {
        Self {
            next_project: 0,
            projects: HashMap::new(),
            new_store: Box::new(move || Ok(new_store())),
        }
    }

    pub fn open_project(&mut self, input: ProjectInput) -> ProjectSessionId {
        self.try_open_project(input)
            .expect("frontend session store factory should create a project store")
    }

    pub fn try_open_project(
        &mut self,
        input: ProjectInput,
    ) -> Result<ProjectSessionId, FrontendSessionError> {
        let store = (self.new_store)()?;
        Ok(self.open_project_with_store(input, store))
    }

    pub fn open_project_with_store(&mut self, input: ProjectInput, store: S) -> ProjectSessionId {
        let id = ProjectSessionId(self.next_project);
        self.next_project += 1;
        self.projects
            .insert(id, FrontendProjectState::new(input, store));
        id
    }

    pub fn apply_changes(
        &mut self,
        project: ProjectSessionId,
        changes: ProjectChangeSet,
    ) -> Result<ChangeSummary, FrontendSessionError> {
        self.project_mut(project)?.apply_changes(changes)
    }

    pub fn check(
        &mut self,
        project: ProjectSessionId,
        request: CheckRequest,
    ) -> Result<CheckResponse, FrontendSessionError> {
        self.project_mut(project)?.check(request)
    }

    pub fn snapshot(&self, project: ProjectSessionId) -> Option<ProjectSemanticSnapshot> {
        self.projects
            .get(&project)
            .and_then(|state| state.last_good_snapshot.clone())
    }

    pub fn source_set(&self, project: ProjectSessionId) -> Option<&SourceSet> {
        self.projects.get(&project).map(|state| &state.sources)
    }

    pub fn artifact_manifest(
        &self,
        project: ProjectSessionId,
    ) -> Option<&FrontendArtifactManifest> {
        self.projects.get(&project).map(|state| &state.manifest)
    }

    fn project_mut(
        &mut self,
        project: ProjectSessionId,
    ) -> Result<&mut FrontendProjectState<S>, FrontendSessionError> {
        self.projects
            .get_mut(&project)
            .ok_or(FrontendSessionError::UnknownProject(project))
    }
}

pub(in crate::session) fn cache_error(error: CacheError) -> FrontendSessionError {
    FrontendSessionError::Cache(error.to_string())
}
