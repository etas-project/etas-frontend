use std::collections::BTreeSet;

use etas_core::Span;
use etas_hir::HirItemId;
use etas_types::EffectArgRef;
use etas_utils::{PassControl, PassFailure, PassManager, PipelineConfig};

use crate::{
    DependencyEffectMetadata, EffectOutput, EffectPipelineError, EffectRegistry, EffectUnit,
    ToolProviderBindingMetadata,
};

mod artifacts;
mod context;
mod passes;

use context::{EffectPipelineContext, effect_pipeline};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalEffectSummaryMetadata {
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub import_root: Option<String>,
    pub item: Vec<String>,
    #[serde(default)]
    pub param_names: Vec<String>,
    #[serde(default)]
    pub type_param_names: Vec<String>,
    #[serde(default)]
    pub effect_param_names: Vec<String>,
    pub public_effects: ExternalEffectRowMetadata,
    pub requested_actions: ExternalEffectRowMetadata,
    #[serde(default)]
    pub handled_requested_actions: ExternalEffectRowMetadata,
    #[serde(default)]
    pub latent_flows: Vec<ExternalLatentFlowSummaryMetadata>,
    #[serde(default)]
    pub action_trace: ExternalActionTraceMetadata,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExternalActionTraceMetadata {
    #[default]
    Empty,
    Event {
        action: ExternalEffectMetadata,
        source: crate::ActionEventSource,
    },
    ParameterCall {
        parameter: String,
    },
    Seq(Vec<ExternalActionTraceMetadata>),
    Choice(Vec<ExternalActionTraceMetadata>),
    Repeat(Box<ExternalActionTraceMetadata>),
    UnknownOrder(Vec<ExternalEffectMetadata>),
    Widened {
        actions: Vec<ExternalEffectMetadata>,
        parameter_calls: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchoredExternalMetadata<T> {
    pub package: u32,
    pub metadata: T,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalArtifactAnchor {
    pub package: u32,
    pub package_label: String,
    pub item: Vec<String>,
    pub span: Span,
}

impl<T> std::ops::Deref for AnchoredExternalMetadata<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalLatentFlowSummaryMetadata {
    #[serde(default)]
    pub declared_bound: ExternalEffectRowMetadata,
    #[serde(default)]
    pub inferred_effects: ExternalEffectRowMetadata,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalEffectRowMetadata {
    #[serde(default)]
    pub effects: Vec<ExternalEffectMetadata>,
    #[serde(default)]
    pub tail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalEffectMetadata {
    pub path: Vec<String>,
    #[serde(default)]
    pub args: Vec<EffectArgRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalTraceSpecSummaryMetadata {
    pub trace_spec: Vec<String>,
    #[serde(default)]
    pub clauses: Vec<ExternalTraceSpecClauseMetadata>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalTraceSpecClauseMetadata {
    pub kind: ExternalTraceSpecClauseKind,
    #[serde(default)]
    pub pattern: Option<ExternalTraceSpecEffectRowMetadata>,
    #[serde(default)]
    pub guard: Option<ExternalTraceSpecEffectRowMetadata>,
    #[serde(default)]
    pub target: Option<ExternalTraceSpecEffectRowMetadata>,
    #[serde(default)]
    pub obligation: Option<ExternalTraceSpecEffectRowMetadata>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExternalTraceSpecClauseKind {
    #[default]
    Allow,
    Deny,
    RequireBefore,
    RequireAfter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalTraceSpecEffectRowMetadata {
    #[serde(default)]
    pub effects: Vec<ExternalTraceSpecEffectMetadata>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalTraceSpecEffectMetadata {
    pub path: Vec<String>,
    #[serde(default)]
    pub args: Vec<etas_types::EffectArgRef>,
}

#[derive(Clone, Copy)]
pub struct EffectPipelineInput<'a> {
    pub hir: &'a etas_hir::HirProgram,
    pub types: &'a etas_types::TypeOutput,
    pub std_registry: &'a etas_std::StdRegistry,
    pub dependency_metadata: Option<&'a DependencyEffectMetadata>,
    pub tool_bindings: &'a [ToolProviderBindingMetadata],
    pub external_summaries: &'a [AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
    pub external_trace_specs: &'a [AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    pub external_artifact_anchors: &'a [ExternalArtifactAnchor],
    pub reachable_items: Option<&'a BTreeSet<HirItemId>>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EffectPipelineArtifacts {
    pub registry: EffectRegistry,
    pub dependency_metadata: DependencyEffectMetadata,
    pub tool_bindings: Vec<ToolProviderBindingMetadata>,
    pub external_summaries: Vec<ExternalEffectSummaryMetadata>,
    pub external_trace_specs: Vec<ExternalTraceSpecSummaryMetadata>,
    pub units: Vec<EffectUnit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_timings: Vec<EffectPipelinePassTiming>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectPipelinePassTiming {
    pub pass: String,
    pub duration_ns: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EffectPipelineOutput {
    pub artifacts: EffectPipelineArtifacts,
    pub effects: EffectOutput,
}

pub struct RunEffectPipeline;

#[derive(Clone, Debug)]
pub struct EffectInferencePlan {
    pub registry: EffectRegistry,
}

impl RunEffectPipeline {
    pub fn schedule_text() -> String {
        let pipeline = effect_pipeline();
        PassManager::<EffectPipelineContext<'static>>::new().schedule_text(&pipeline)
    }

    pub fn artifacts(
        input: EffectPipelineInput<'_>,
    ) -> Result<EffectPipelineArtifacts, EffectPipelineError> {
        Ok(Self::run(input)?.artifacts)
    }

    pub fn run(
        input: EffectPipelineInput<'_>,
    ) -> Result<EffectPipelineOutput, EffectPipelineError> {
        let mut context = EffectPipelineContext::new(input);
        let mut pipeline = effect_pipeline();
        let mut manager = PassManager::with_config(PipelineConfig::default().with_timing(true));
        let result = manager.run_global_pipeline(&mut pipeline, &mut context);
        match &result.control {
            PassControl::Continue => {}
            PassControl::Stop => return Err(EffectPipelineError::Stopped),
            PassControl::Failed(failure) => {
                return Err(context
                    .failure
                    .take()
                    .unwrap_or_else(|| pass_manager_failure(failure)));
            }
        }
        let mut output = context.into_output()?;
        output.artifacts.pass_timings = result
            .records
            .into_iter()
            .filter_map(|record| {
                let timing = record.timing?;
                Some(EffectPipelinePassTiming {
                    pass: timing.pass.to_owned(),
                    duration_ns: timing.duration.as_nanos().min(u128::from(u64::MAX)) as u64,
                })
            })
            .collect();
        Ok(output)
    }

    pub fn plan(
        input: EffectPipelineInput<'_>,
    ) -> Result<EffectInferencePlan, EffectPipelineError> {
        Ok(Self::artifacts(input)?.into())
    }
}

fn pass_manager_failure(failure: &PassFailure) -> EffectPipelineError {
    EffectPipelineError::PassManagerFailure {
        reason: failure.message.clone(),
        missing_artifact: failure.missing_artifact,
    }
}

impl From<EffectPipelineArtifacts> for EffectInferencePlan {
    fn from(artifacts: EffectPipelineArtifacts) -> Self {
        Self {
            registry: artifacts.registry,
        }
    }
}

#[cfg(test)]
mod tests {
    use etas_hir::HirProgram;
    use etas_types::TypeOutput;
    use etas_utils::{ArtifactKey, PassControl, PassManager, Pipeline};

    use super::*;
    use crate::pipeline::artifacts::EFFECT_REGISTRY;
    use crate::pipeline::context::EffectPipelineContext;
    use crate::pipeline::passes::SolveSummariesPass;

    #[test]
    fn pass_manager_failure_preserves_missing_artifact_without_fabricating_stage() {
        let artifact = ArtifactKey::new("effects", "registry");
        let failure = PassFailure::missing_artifact(artifact);

        assert_eq!(
            pass_manager_failure(&failure),
            EffectPipelineError::PassManagerFailure {
                reason: failure.message,
                missing_artifact: Some(artifact),
            }
        );
    }

    #[test]
    fn pass_manager_rejects_effect_stage_when_required_artifact_is_absent() {
        let hir = HirProgram::default();
        let types = TypeOutput::default();
        let input = EffectPipelineInput {
            hir: &hir,
            types: &types,
            std_registry: &etas_std::standard_registry(),
            dependency_metadata: None,
            tool_bindings: &[],
            external_summaries: &[],
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        };
        let mut context = EffectPipelineContext::new(input);
        let mut pipeline = Pipeline::new("effects.missing_artifact").pass(SolveSummariesPass);
        let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);

        let PassControl::Failed(failure) = result.control else {
            panic!("effect pass manager accepted a stage without its required artifacts");
        };
        assert_eq!(failure.missing_artifact, Some(EFFECT_REGISTRY));
        assert!(context.analysis.is_none());
    }
}
