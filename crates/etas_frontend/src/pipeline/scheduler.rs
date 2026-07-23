use etas_core::SourceId;
use etas_utils::{PassControl, PassManager, PassRunRecord, PipelineConfig};

use crate::incremental::{BodyArtifactReuseInput, CheckScope};
use crate::{ParsedSource, ProjectContext, ProjectInput, ProjectOutput};
use std::{collections::HashMap, sync::Arc};

use super::adapters::project_output_from_context;
use super::passes::project_pipeline;

pub(crate) struct FrontendPipelineRun {
    pub(crate) output: ProjectOutput,
    pub(crate) reused_artifacts: Vec<etas_cache::ArtifactKey>,
    pub(crate) control: PassControl,
    pub(crate) records: Vec<PassRunRecord>,
}

pub(crate) struct FrontendPipelineRequest {
    pub(crate) input: ProjectInput,
    pub(crate) incremental: bool,
    pub(crate) changed_sources: Vec<SourceId>,
    pub(crate) project_wide_change: bool,
    pub(crate) body_artifact_reuse: BodyArtifactReuseInput,
    pub(crate) parsed_source_reuse: HashMap<SourceId, ParsedSource>,
    pub(crate) check_scope: CheckScope,
    pub(crate) collect_timing: bool,
    pub(crate) std_registry: Arc<etas_std::StdRegistry>,
}

pub(crate) fn run_check_pipeline(request: FrontendPipelineRequest) -> FrontendPipelineRun {
    let FrontendPipelineRequest {
        input,
        incremental,
        changed_sources,
        project_wide_change,
        body_artifact_reuse,
        parsed_source_reuse,
        check_scope,
        collect_timing,
        std_registry,
    } = request;
    let mut context = if incremental {
        ProjectContext::new_incremental_with_reuse_and_std_registry(
            input,
            changed_sources,
            project_wide_change,
            body_artifact_reuse,
            parsed_source_reuse,
            std_registry,
        )
    } else {
        ProjectContext::new_with_std_registry(input, std_registry)
    }
    .with_check_scope(check_scope);
    let mut pipeline = project_pipeline();
    let mut manager =
        PassManager::with_config(PipelineConfig::default().with_timing(collect_timing));
    let result = manager.run_pipeline(&mut pipeline, &mut context);
    let control = result.control.clone();
    let records = result.records;
    let reused_artifacts = context.reused_cache_artifacts.clone();
    let output = project_output_from_context(context);
    FrontendPipelineRun {
        output,
        reused_artifacts,
        control,
        records,
    }
}
