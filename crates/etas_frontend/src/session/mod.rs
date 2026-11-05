use std::{collections::HashMap, path::PathBuf, sync::Arc};

use etas_cache::{CacheError, MemoryArtifactStore, ProjectRevision};
use etas_core::{SourceId, id_type};

use crate::artifact::FrontendArtifactManifest;
use crate::incremental::{
    ChangeSummary, CheckRequest, CheckResponse, ProjectChangeSet, SourceVersion,
};
use crate::{ExternalPackageId, ProjectInput, SourceSet};

mod change;
mod delta;
mod memory;
mod options;
#[cfg(test)]
mod ownership_tests;
mod project;
mod snapshot;
mod source_state;
mod store;

use project::FrontendProjectState;

pub use delta::{DiagnosticDelta, ProjectSemanticDelta};
pub(crate) use memory::MemoryInstrumentation;
pub use memory::{
    AllocationSnapshot, CheckMemoryReport, FrontendMemoryProbe, PassMemorySample,
    ProjectMemoryUsage,
};
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
    InvalidOverlaySourceIdentity(SourceId),
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
    std_registry: Arc<etas_std::StdRegistry>,
    memory_probe: Option<FrontendMemoryProbe>,
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

    pub fn with_std_registry(std_registry: Arc<etas_std::StdRegistry>) -> Self {
        Self {
            next_project: 0,
            projects: HashMap::new(),
            new_store: Box::new(|| Ok(MemoryArtifactStore::new())),
            std_registry,
            memory_probe: None,
        }
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
            std_registry: Arc::new(etas_std::standard_registry()),
            memory_probe: None,
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
            std_registry: Arc::new(etas_std::standard_registry()),
            memory_probe: None,
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
        self.projects.insert(
            id,
            FrontendProjectState::new(input, store, self.std_registry.clone()),
        );
        id
    }

    pub fn apply_changes(
        &mut self,
        project: ProjectSessionId,
        changes: ProjectChangeSet,
    ) -> Result<ChangeSummary, FrontendSessionError> {
        self.project_mut(project)?.apply_changes(changes)
    }

    pub fn replace_compile_options(
        &mut self,
        project: ProjectSessionId,
        revision: ProjectRevision,
        options: crate::ProjectCompileOptions,
    ) -> Result<ChangeSummary, FrontendSessionError> {
        self.project_mut(project)?
            .replace_compile_options(revision, options)
    }

    pub fn check(
        &mut self,
        project: ProjectSessionId,
        request: CheckRequest,
    ) -> Result<CheckResponse, FrontendSessionError> {
        let probe = self.memory_probe;
        self.project_mut(project)?
            .check_with_memory_probe(request, probe)
    }

    pub fn check_controlled(
        &mut self,
        project: ProjectSessionId,
        request: CheckRequest,
        control: Arc<dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<CheckResponse, FrontendSessionError> {
        let probe = self.memory_probe;
        self.project_mut(project)?
            .check_controlled_with_memory_probe(request, probe, control)
    }

    pub fn set_memory_probe(&mut self, probe: Option<FrontendMemoryProbe>) {
        self.memory_probe = probe;
    }

    pub fn analyze_body_overlay(
        &mut self,
        overlay: crate::code::draft::DefaultBodyOverlay,
    ) -> Result<
        crate::code::service::CandidateBodyAnalysis,
        crate::code::service::CandidateAnalysisError,
    > {
        crate::code::pipeline::analyze_body(overlay, self.memory_probe)
    }

    pub fn analyze_body_overlay_controlled(
        &mut self,
        overlay: crate::code::draft::DefaultBodyOverlay,
        control: Arc<dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<
        crate::code::service::CandidateBodyAnalysis,
        crate::code::service::CandidateAnalysisError,
    > {
        crate::code::pipeline::analyze_controlled_body(overlay, self.memory_probe, Some(control))
    }

    pub fn compile_body_overlay(
        &mut self,
        overlay: crate::code::draft::DefaultBodyOverlay,
    ) -> Result<crate::code::service::CheckedCandidate, crate::code::service::CandidateCompileError>
    {
        self.analyze_body_overlay(overlay)
            .map_err(crate::code::service::CandidateCompileError::Analysis)?
            .into_checked_candidate(Default::default())
    }

    pub fn analyze_body_overlay_with_dependencies(
        &mut self,
        overlay: crate::code::draft::DefaultBodyOverlay,
        dependency_sources: Vec<crate::code::service::CandidateDependencySource>,
        descriptor_limits: etas_code::descriptor::DescriptorLimits,
        control: Option<Arc<dyn etas_utils::PipelineExecutionControl>>,
    ) -> Result<
        crate::code::service::CandidateBodyAnalysis,
        crate::code::service::CandidateAnalysisError,
    > {
        crate::code::pipeline::analyze_body_with_dependencies(
            overlay,
            dependency_sources,
            descriptor_limits,
            self.memory_probe,
            control,
        )
    }

    pub fn snapshot(&self, project: ProjectSessionId) -> Option<ProjectSemanticSnapshot> {
        self.projects
            .get(&project)
            .and_then(|state| state.last_good_snapshot.clone())
    }

    pub fn memory_usage(&self, project: ProjectSessionId) -> Option<ProjectMemoryUsage> {
        self.projects
            .get(&project)
            .map(|state| state.memory_usage())
    }

    pub fn close_project(&mut self, project: ProjectSessionId) -> Result<(), FrontendSessionError> {
        self.projects
            .remove(&project)
            .map(drop)
            .ok_or(FrontendSessionError::UnknownProject(project))
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

#[cfg(test)]
mod optimization_level_tests {
    use super::FrontendSession;
    use crate::{CheckRequest, FrontendArtifactKind, OptimizationLevel, ProjectInput, SourceInput};

    #[test]
    fn live_level_change_reuses_pre_optimization_work_and_rebuilds_checked_hir() {
        let mut input =
            ProjectInput::single_source(SourceInput::anonymous("flow main() -> i64 { return 1; }"));
        input.options.optimization_level = OptimizationLevel::O0;
        let mut session = FrontendSession::new();
        let project = session.open_project(input);
        let initial = session
            .check(project, CheckRequest::full_project())
            .unwrap();
        let initial_checked = initial.output.checked.as_ref().unwrap();
        assert_eq!(
            initial_checked.compile_options.optimization_level,
            OptimizationLevel::O0
        );
        assert!(initial_checked.optimization_report.passes.is_empty());

        let mut options = initial_checked.compile_options.clone();
        options.optimization_level = OptimizationLevel::O3;
        let change = session
            .replace_compile_options(project, etas_cache::ProjectRevision(1), options)
            .unwrap();
        assert_eq!(
            session.projects[&project]
                .pending_body_artifact_reuse
                .type_outputs
                .len(),
            1
        );
        assert!(
            change.invalidated_artifacts.iter().any(|artifact| {
                artifact.kind.as_str() == FrontendArtifactKind::HirProgram.key()
            })
        );
        assert!(!change.invalidated_artifacts.iter().any(|artifact| {
            artifact.kind.as_str() == FrontendArtifactKind::ParsedSource.key()
                || artifact.kind.as_str() == FrontendArtifactKind::ModuleIndex.key()
        }));

        let rebuilt = session.check(project, CheckRequest::incremental()).unwrap();
        let rebuilt_checked = rebuilt.output.checked.as_ref().unwrap();
        assert_eq!(
            rebuilt_checked.compile_options.optimization_level,
            OptimizationLevel::O3
        );
        assert_eq!(
            rebuilt_checked.optimization_report.effective_level,
            OptimizationLevel::O3
        );
        assert!(
            rebuilt_checked
                .optimization_report
                .pass("LowerResourceRegion")
                .is_some()
        );
        assert_eq!(rebuilt.output.reused_parsed_sources, 1);
        assert!(
            rebuilt
                .pipeline_records
                .iter()
                .filter(|record| record.pass == "TypeCheckBodyPass")
                .all(|record| !record.changed)
        );
        assert!(
            rebuilt.cache.reused_artifacts.iter().any(|artifact| {
                artifact.kind.as_str() == FrontendArtifactKind::TypeFacts.key()
            })
        );
    }
}
