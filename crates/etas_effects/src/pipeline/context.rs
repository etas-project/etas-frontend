use etas_core::Diagnostic;
use etas_hir_analysis::HirAnalysisContext;
use etas_utils::{PassResult, Pipeline};

use crate::{
    EffectAnalysisOutput, EffectOutput, EffectPipelineError, EffectRegistry, EffectStage,
    EffectUnit,
};

use super::passes::build_effect_pipeline;
use super::{EffectPipelineArtifacts, EffectPipelineInput, EffectPipelineOutput};

pub(super) struct EffectPipelineContext<'a> {
    pub(super) input: EffectPipelineInput<'a>,
    pub(super) analysis_context: HirAnalysisContext,
    pub(super) registry: Option<EffectRegistry>,
    pub(super) units: Option<Vec<EffectUnit>>,
    pub(super) memory_provenance:
        Option<std::sync::Arc<crate::infer::memory_provenance::MemoryProvenance>>,
    pub(super) registry_diagnostics: Vec<Diagnostic>,
    pub(super) artifacts: Option<EffectPipelineArtifacts>,
    pub(super) analysis: Option<EffectAnalysisOutput>,
    pub(super) effects: Option<EffectOutput>,
    pub(super) trace_spec_models: Option<crate::trace_spec::TraceSpecModelStore>,
    pub(super) trace_spec_analysis: Option<crate::trace_spec::TraceSpecAnalysisOutput>,
    pub(super) failure: Option<EffectPipelineError>,
}

impl<'a> EffectPipelineContext<'a> {
    pub(super) fn new(input: EffectPipelineInput<'a>) -> Self {
        Self {
            analysis_context: HirAnalysisContext::new(input.hir),
            input,
            registry: None,
            units: None,
            memory_provenance: None,
            registry_diagnostics: Vec::new(),
            artifacts: None,
            analysis: None,
            effects: None,
            trace_spec_models: None,
            trace_spec_analysis: None,
            failure: None,
        }
    }

    pub(super) fn into_output(self) -> Result<EffectPipelineOutput, EffectPipelineError> {
        Ok(EffectPipelineOutput {
            artifacts: self
                .artifacts
                .ok_or(EffectPipelineError::MissingStageArtifact {
                    stage: EffectStage::Emit,
                    artifact: "pipeline artifacts",
                })?,
            effects: self
                .effects
                .ok_or(EffectPipelineError::MissingStageArtifact {
                    stage: EffectStage::Emit,
                    artifact: "effect output",
                })?,
        })
    }

    pub(super) fn fail(&mut self, error: EffectPipelineError) -> PassResult {
        let message = error.to_string();
        self.failure = Some(error);
        PassResult::failed(message)
    }
}

pub(super) fn effect_pipeline<'a>() -> Pipeline<EffectPipelineContext<'a>> {
    build_effect_pipeline()
}
