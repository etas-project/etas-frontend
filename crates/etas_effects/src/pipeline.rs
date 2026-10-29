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
mod prepared_registry;
pub use prepared_registry::PreparedEffectRegistry;

use context::{EffectPipelineContext, effect_pipeline};

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalEffectSummaryMetadata {
    /// Validated portable analysis for this exact declaration, when published.
    #[serde(default)]
    pub callable_contract: Option<ExternalCallableContract>,
    #[serde(default)]
    pub behavior: Option<etas_package_metadata::CallableValueBehavior>,
    pub handler_requests: Option<ExternalRequestInstantiationMetadata>,
    pub callable_graph_key: Option<etas_code::CodeFingerprint>,
    pub returned_callables: Option<etas_package_metadata::CallableValueGraph>,
    pub callable_values: Vec<ExternalCallableValueMetadata>,
    #[serde(default)]
    pub default_handler_effects: Option<ExternalDefaultHandlerEffectsMetadata>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub import_root: Option<String>,
    pub item: Vec<String>,
    #[serde(default)]
    pub param_names: Vec<String>,
    #[serde(default)]
    pub generic_params: Vec<(String, etas_types::CallableGenericParamKind)>,
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

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalCallableContract {
    pub header: std::sync::Arc<etas_code::descriptor::CallableHeaderInfo>,
    pub graph: std::sync::Arc<etas_code::descriptor::DescriptorGraphInput>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalCallableValueMetadata {
    pub handler_requests: Option<ExternalRequestInstantiationMetadata>,
    pub params: Vec<etas_types::TypeId>,
    pub output: etas_types::TypeId,
    pub public_effects: ExternalEffectRowMetadata,
    pub requested_actions: ExternalEffectRowMetadata,
    pub handled_requested_actions: ExternalEffectRowMetadata,
    pub default_handler_effects: ExternalDefaultHandlerEffectsMetadata,
    pub action_trace: ExternalActionTraceMetadata,
}

pub(crate) struct ExternalCallableEffectView<'a> {
    pub handler_requests: Option<&'a ExternalRequestInstantiationMetadata>,
    pub parameter_targets: &'a [(String, crate::ParameterCallTarget)],
    pub public_effects: &'a ExternalEffectRowMetadata,
    pub requested_actions: &'a ExternalEffectRowMetadata,
    pub handled_requested_actions: &'a ExternalEffectRowMetadata,
    pub default_handler_effects: Option<&'a ExternalDefaultHandlerEffectsMetadata>,
    pub action_trace: &'a ExternalActionTraceMetadata,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalRequestInstantiationMetadata {
    pub actions: Vec<ExternalActionRequestMetadata>,
    pub parameters: Vec<ExternalParameterRequestMetadata>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalActionRequestMetadata {
    pub action: ExternalEffectMetadata,
    pub type_bindings: Option<Vec<(String, etas_types::TypeId)>>,
    pub signature: Option<etas_types::ActionRequestSignature>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalParameterRequestMetadata {
    pub parameter: String,
    pub excluded: Vec<ExternalEffectMetadata>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalDefaultHandlerEffectsMetadata {
    pub direct_effects: ExternalEffectRowMetadata,
    pub default_actions: ExternalEffectRowMetadata,
    pub handlers: Vec<ExternalDefaultHandlerContributionMetadata>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalDefaultHandlerContributionMetadata {
    pub action: ExternalEffectMetadata,
    pub produced: ExternalEffectRowMetadata,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
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

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct AnchoredExternalMetadata<T> {
    pub package: u32,
    pub metadata: T,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
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

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalLatentFlowSummaryMetadata {
    #[serde(default)]
    pub declared_bound: ExternalEffectRowMetadata,
    #[serde(default)]
    pub inferred_effects: ExternalEffectRowMetadata,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalEffectRowMetadata {
    #[serde(default)]
    pub effects: Vec<ExternalEffectMetadata>,
    #[serde(default)]
    pub tail: Option<String>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalEffectMetadata {
    pub path: Vec<String>,
    #[serde(default)]
    pub args: Vec<EffectArgRef>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalTraceSpecSummaryMetadata {
    pub trace_spec: Vec<String>,
    #[serde(default)]
    pub clauses: Vec<ExternalTraceSpecClauseMetadata>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
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

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum ExternalTraceSpecClauseKind {
    #[default]
    Allow,
    Deny,
    RequireBefore,
    RequireAfter,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalTraceSpecEffectRowMetadata {
    #[serde(default)]
    pub effects: Vec<ExternalTraceSpecEffectMetadata>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ExternalTraceSpecEffectMetadata {
    pub path: Vec<String>,
    #[serde(default)]
    pub args: Vec<etas_types::EffectArgRef>,
}

#[derive(Clone, Copy, etas_core::RetainedSize)]
pub struct EffectPipelineInput<'a> {
    pub frozen_calls: Option<&'a dyn crate::FrozenCallableSummaryProvider>,
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeOutput,
    pub std_registry: &'a etas_std::StdRegistry,
    pub dependency_metadata: Option<&'a DependencyEffectMetadata>,
    pub tool_bindings: &'a [ToolProviderBindingMetadata],
    pub external_summaries: &'a [AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
    pub external_trace_specs: &'a [AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    pub external_artifact_anchors: &'a [ExternalArtifactAnchor],
    pub reachable_items: Option<&'a BTreeSet<HirItemId>>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectPipelineArtifacts {
    pub registry: std::sync::Arc<EffectRegistry>,
    pub dependency_metadata: DependencyEffectMetadata,
    pub tool_bindings: Vec<ToolProviderBindingMetadata>,
    pub external_summaries: Vec<ExternalEffectSummaryMetadata>,
    pub external_trace_specs: Vec<ExternalTraceSpecSummaryMetadata>,
    pub units: Vec<EffectUnit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_timings: Vec<EffectPipelinePassTiming>,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct EffectPipelinePassTiming {
    pub pass: String,
    pub duration_ns: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectPipelineOutput {
    pub artifacts: EffectPipelineArtifacts,
    pub effects: EffectOutput,
}

pub struct RunEffectPipeline;

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct EffectInferencePlan {
    pub registry: std::sync::Arc<EffectRegistry>,
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
        Self::run_with_control(input, None, context::RegistrySource::CheckedTypes)
    }

    pub fn run_controlled(
        input: EffectPipelineInput<'_>,
        control: std::sync::Arc<dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<EffectPipelineOutput, EffectPipelineError> {
        Self::run_with_control(input, Some(control), context::RegistrySource::CheckedTypes)
    }

    pub fn run_prepared(
        input: EffectPipelineInput<'_>,
        prepared: PreparedEffectRegistry,
        control: Option<std::sync::Arc<dyn etas_utils::PipelineExecutionControl>>,
    ) -> Result<EffectPipelineOutput, EffectPipelineError> {
        Self::run_with_control(
            input,
            control,
            context::RegistrySource::Prepared(Box::new(prepared)),
        )
    }

    fn run_with_control(
        input: EffectPipelineInput<'_>,
        control: Option<std::sync::Arc<dyn etas_utils::PipelineExecutionControl>>,
        registry_source: context::RegistrySource,
    ) -> Result<EffectPipelineOutput, EffectPipelineError> {
        let mut context = EffectPipelineContext::new(input);
        context.registry_source = registry_source;
        let mut pipeline = effect_pipeline();
        let mut manager = PassManager::with_config(PipelineConfig::default().with_timing(true));
        if let Some(control) = control {
            manager.set_execution_control(control);
        }
        let result = manager.run_global_pipeline(&mut pipeline, &mut context);
        match &result.control {
            PassControl::Continue => {}
            PassControl::Stop => return Err(EffectPipelineError::Stopped),
            PassControl::Failed(failure) => {
                if failure.interruption.is_some() {
                    return Err(pass_manager_failure(failure));
                }
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
    if let Some(interruption) = &failure.interruption {
        return EffectPipelineError::Interrupted(interruption.clone());
    }
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
    use crate::pipeline::artifacts::{EFFECT_REGISTRY, FROZEN_CALL_SUMMARIES, MEMORY_PROVENANCE};
    use crate::pipeline::context::EffectPipelineContext;
    use crate::pipeline::passes::{
        AnalyzeMemoryProvenancePass, BindFrozenCallsPass, BuildRegistryPass, CollectUnitsPass,
        SolveSummariesPass,
    };

    #[test]
    fn unvalidated_hir_cannot_enter_effect_pipeline() {
        let mut hir = HirProgram::default();
        hir.modules.push(etas_hir::HirModuleId(0));
        let before = etas_hir::HirTreeIndex::thread_build_count();
        let result = etas_hir::HirRevision::try_new(hir);
        assert!(result.is_err());
        assert_eq!(etas_hir::HirTreeIndex::thread_build_count() - before, 1);
    }

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
        for missing in [EFFECT_REGISTRY, MEMORY_PROVENANCE, FROZEN_CALL_SUMMARIES] {
            let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
            let types = TypeOutput::default();
            let input = EffectPipelineInput {
                frozen_calls: None,
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
            let pipeline = Pipeline::new("effects.missing_artifact").pass(CollectUnitsPass);
            let mut pipeline = if missing == EFFECT_REGISTRY {
                pipeline
                    .pass(BindFrozenCallsPass)
                    .pass(AnalyzeMemoryProvenancePass)
            } else if missing == FROZEN_CALL_SUMMARIES {
                pipeline
                    .pass(BuildRegistryPass)
                    .pass(AnalyzeMemoryProvenancePass)
            } else {
                pipeline.pass(BuildRegistryPass).pass(BindFrozenCallsPass)
            }
            .pass(SolveSummariesPass);
            let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);

            let PassControl::Failed(failure) = result.control else {
                panic!("effect pass manager accepted a stage without its required artifacts");
            };
            assert_eq!(failure.missing_artifact, Some(missing));
            assert!(context.analysis.is_none());
        }
    }

    struct CancelAnalysis;

    impl etas_utils::PipelineExecutionControl for CancelAnalysis {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "cancel inside effect summary analysis".into(),
            })
        }
    }

    #[test]
    fn summary_stage_propagates_typed_cancellation_without_installing_partial_facts() {
        use etas_utils::{Pass, PassContext};

        let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
        let types = TypeOutput::default();
        let registry = etas_std::standard_registry();
        let input = EffectPipelineInput {
            frozen_calls: None,
            hir: &hir,
            types: &types,
            std_registry: &registry,
            dependency_metadata: None,
            tool_bindings: &[],
            external_summaries: &[],
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        };
        let mut context = EffectPipelineContext::new(input);
        let mut prefix = Pipeline::new("effects.summary_inputs")
            .pass(BuildRegistryPass)
            .pass(BindFrozenCallsPass)
            .pass(CollectUnitsPass)
            .pass(AnalyzeMemoryProvenancePass);
        let mut manager = PassManager::new();
        assert!(matches!(
            manager
                .run_global_pipeline(&mut prefix, &mut context)
                .control,
            PassControl::Continue
        ));
        // Invoke the stage itself so a pass-boundary check cannot satisfy this test.
        manager.set_execution_control(std::sync::Arc::new(CancelAnalysis));
        let result = SolveSummariesPass.run(&mut context, &PassContext::new(None), &mut manager);
        let PassControl::Failed(failure) = result.control else {
            panic!("expected cancellation")
        };
        assert_eq!(
            failure.interruption.unwrap().kind,
            etas_utils::PipelineInterruptionKind::Cancelled
        );
        assert!(matches!(
            context.failure,
            Some(EffectPipelineError::Interrupted(_))
        ));
        assert!(context.analysis.is_none());
        assert!(context.effects.is_none());
        assert!(result.produced.is_empty());
    }
}
