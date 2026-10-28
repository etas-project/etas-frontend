use etas_core::SourceId;
use etas_utils::{PassControl, PassManager, PassRunRecord, Pipeline, PipelineConfig};

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
    pub(crate) memory_samples: Vec<crate::session::PassMemorySample>,
    pub(crate) candidate_contract: Option<crate::code::contract::CandidateContractAnalysis>,
    pub(crate) candidate_emission: Option<crate::code::service::CandidateEmission>,
    pub(crate) candidate_descriptors: Option<crate::code::pipeline::passes::CandidateDescriptors>,
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
    pub(crate) memory_probe: Option<crate::session::FrontendMemoryProbe>,
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
        memory_probe,
    } = request;
    let context = if incremental {
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
    run_context_pipeline(context, project_pipeline(), collect_timing, memory_probe)
}

pub(crate) fn run_context_pipeline(
    context: ProjectContext,
    pipeline: Pipeline<ProjectContext>,
    collect_timing: bool,
    memory_probe: Option<crate::session::FrontendMemoryProbe>,
) -> FrontendPipelineRun {
    run_controlled_context_pipeline(context, pipeline, collect_timing, memory_probe, None)
}

pub(crate) fn run_controlled_context_pipeline(
    mut context: ProjectContext,
    mut pipeline: Pipeline<ProjectContext>,
    collect_timing: bool,
    memory_probe: Option<crate::session::FrontendMemoryProbe>,
    control: Option<std::sync::Arc<dyn etas_utils::PipelineExecutionControl>>,
) -> FrontendPipelineRun {
    let mut manager =
        PassManager::with_config(PipelineConfig::default().with_timing(collect_timing));
    if let Some(control) = control {
        manager.set_execution_control(control);
    }
    let memory = memory_probe.map(crate::session::MemoryInstrumentation::new);
    if let Some(hook) = &memory {
        manager.add_instrumentation(hook.clone());
    }
    let result = manager.run_pipeline(&mut pipeline, &mut context);
    let control = result.control.clone();
    let records = result.records;
    let reused_artifacts = context.reused_cache_artifacts.clone();
    let candidate_contract = context
        .candidate
        .as_mut()
        .and_then(|candidate| candidate.contract_analysis.take());
    let candidate_emission = context
        .candidate
        .as_mut()
        .and_then(|candidate| candidate.emission.take());
    let candidate_descriptors = context
        .candidate
        .as_mut()
        .and_then(|candidate| candidate.descriptors.take());
    let output = project_output_from_context(context);
    FrontendPipelineRun {
        output,
        reused_artifacts,
        control,
        records,
        memory_samples: memory.map(|hook| hook.take_samples()).unwrap_or_default(),
        candidate_contract,
        candidate_emission,
        candidate_descriptors,
    }
}
