use etas_core::SourceId;
use etas_utils::{PassControl, PassManager, PassRunRecord, PipelineConfig};

use crate::incremental::{BodyArtifactReuseInput, CheckScope};
use crate::{ParsedSource, ProjectContext, ProjectInput, ProjectOutput};
use std::collections::HashMap;

use super::adapters::project_output_from_context;
use super::passes::project_pipeline;

pub(crate) struct FrontendPipelineRun {
    pub(crate) output: ProjectOutput,
    pub(crate) reused_artifacts: Vec<etas_cache::ArtifactKey>,
    pub(crate) control: PassControl,
    pub(crate) records: Vec<PassRunRecord>,
}

pub(crate) fn run_check_pipeline(
    input: ProjectInput,
    incremental: bool,
    changed_sources: Vec<SourceId>,
    project_wide_change: bool,
    body_artifact_reuse: BodyArtifactReuseInput,
    parsed_source_reuse: HashMap<SourceId, ParsedSource>,
    check_scope: CheckScope,
    collect_timing: bool,
) -> FrontendPipelineRun {
    let mut context = if incremental {
        ProjectContext::new_incremental_with_reuse(
            input,
            changed_sources,
            project_wide_change,
            body_artifact_reuse,
            parsed_source_reuse,
        )
    } else {
        ProjectContext::new(input)
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
