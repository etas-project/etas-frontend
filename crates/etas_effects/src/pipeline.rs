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
    pub object_values: Vec<ExternalExistentialValueMetadata>,
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
    pub descriptor_fingerprint: etas_code::CodeFingerprint,
    pub header: std::sync::Arc<etas_code::descriptor::CallableHeaderInfo>,
    pub graph: std::sync::Arc<etas_code::descriptor::DescriptorGraphInput>,
    pub definition_paths:
        std::sync::Arc<std::collections::BTreeMap<etas_code::DefinitionKey, Vec<Vec<String>>>>,
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

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalExistentialValueMetadata {
    pub object_type: etas_types::TypeId,
    pub source_type: etas_types::TypeId,
    pub witnesses: Vec<ExternalExistentialMemberMetadata>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalExistentialMemberMetadata {
    pub bound: etas_types::CheckedSpecBound,
    pub member: etas_types::SpecMethodIdentity,
    pub target: etas_package_metadata::ExistentialWitnessTarget,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalSpecImplementationMetadata {
    pub identity: etas_code::MethodId,
    pub owner_params: Vec<etas_types::CallableGenericParam>,
    pub graph_key: etas_code::CodeFingerprint,
    pub graph: std::sync::Arc<etas_package_metadata::CallableValueGraph>,
    pub values: Vec<ExternalCallableValueMetadata>,
    pub objects: Vec<ExternalExistentialValueMetadata>,
    pub callable_contract: Option<ExternalCallableContract>,
    pub requirements: ExternalMethodRequirements,
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
pub struct ExternalMethodRequirements {
    pub limits: Vec<crate::LimitRequirement>,
    pub trace_requirements: Vec<ExternalMethodTraceRequirement>,
    pub trace_obligations: Vec<ExternalMethodTraceRequirement>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ExternalMethodTraceRequirement {
    Clause(ExternalTraceSpecClauseMetadata),
    Reference(String),
    Limit(usize),
}

pub(crate) struct ExternalCallableEffectView<'a> {
    pub generic_params: &'a [(String, etas_types::CallableGenericParamKind)],
    pub handler_requests: Option<&'a ExternalRequestInstantiationMetadata>,
    pub parameter_targets: &'a [(String, crate::ParameterCallTarget)],
    pub public_effects: &'a ExternalEffectRowMetadata,
    pub requested_actions: &'a ExternalEffectRowMetadata,
    pub handled_requested_actions: &'a ExternalEffectRowMetadata,
    pub default_handler_effects: Option<&'a ExternalDefaultHandlerEffectsMetadata>,
    pub action_trace: &'a ExternalActionTraceMetadata,
    pub symbolic_parameter_bounds: &'a [crate::EffectRow],
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
    pub parameter: ExternalCallableRequestTarget,
    pub excluded: Vec<ExternalEffectMetadata>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ExternalCallableRequestTarget {
    Parameter(String),
    SpecMethod(std::sync::Arc<ExternalSpecMethodInvocation>),
}

impl From<&str> for ExternalCallableRequestTarget {
    fn from(value: &str) -> Self {
        Self::Parameter(value.into())
    }
}
impl From<String> for ExternalCallableRequestTarget {
    fn from(value: String) -> Self {
        Self::Parameter(value)
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalSpecMethodInvocation {
    pub identity: etas_code::CodeFingerprint,
    pub receiver: etas_types::TypeId,
    pub spec: etas_types::ExternalSpecIdentity,
    pub spec_args: Vec<etas_types::TypeId>,
    pub method: String,
    pub arity: u32,
    pub generic_arguments: Vec<ExternalSpecMethodGenericArgument>,
    pub callable_arguments: Vec<ExternalSpecMethodCallableArgument>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ExternalSpecMethodGenericArgument {
    Type(etas_types::TypeId),
    EffectRow(etas_types::EffectRowRef),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalSpecMethodCallableArgument {
    pub position: u32,
    pub sources: etas_package_metadata::CallableValueGraph,
    pub values: Vec<ExternalCallableValueMetadata>,
    pub objects: Vec<ExternalExistentialValueMetadata>,
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
        parameter: ExternalCallableRequestTarget,
    },
    Seq(Vec<ExternalActionTraceMetadata>),
    Choice(Vec<ExternalActionTraceMetadata>),
    Repeat(Box<ExternalActionTraceMetadata>),
    UnknownOrder(Vec<ExternalEffectMetadata>),
    Widened {
        actions: Vec<ExternalEffectMetadata>,
        parameter_calls: Vec<ExternalCallableRequestTarget>,
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
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ExternalTraceSpecSummaryMetadata {
    pub trace_spec: Vec<String>,
    pub definition: etas_code::DefinitionKey,
    pub alternatives: Vec<Vec<ExternalTraceSpecClauseMetadata>>,
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
    pub source_snapshots: &'a [EffectSourceSnapshot<'a>],
    pub residual_identity: Option<&'a dyn crate::ResidualIdentityProvider>,
    /// Checked canonical projection used to lower residual monitor plans.
    /// Without it no plan is produced and unproven conformances stay rejected.
    pub trace_monitor_projector: Option<&'a dyn crate::TraceMonitorProjector>,
    pub external_implementations:
        &'a [AnchoredExternalMetadata<ExternalSpecImplementationMetadata>],
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

/// Source snapshots supplied by the project owner; these are identity inputs,
/// not proof of residual plan coverage or runtime installation.
#[derive(Clone, Copy)]
pub struct EffectSourceSnapshot<'a> {
    pub source: etas_core::SourceId,
    pub key: etas_code::StableSourceKey,
    pub text: &'a str,
}

pub(crate) fn validate_source_snapshot_identities(
    snapshots: &[EffectSourceSnapshot<'_>],
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(), EffectPipelineError> {
    let limit = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    if snapshots.len() > limit {
        return Err(EffectPipelineError::AnalysisLimitExceeded {
            resource: "effect source snapshots",
            limit,
        });
    }
    let mut source_ids = BTreeSet::new();
    let mut source_keys = BTreeSet::new();
    for snapshot in snapshots {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        if !source_ids.insert(snapshot.source.0) {
            return Err(EffectPipelineError::InvalidSourceIdentity {
                source: snapshot.source,
                reason: "duplicate source snapshot".into(),
            });
        }
        if !source_keys.insert(snapshot.key) {
            return Err(EffectPipelineError::InvalidSourceIdentity {
                source: snapshot.source,
                reason: "stable source key belongs to multiple snapshots".into(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct EffectPipelineArtifacts {
    pub external_implementations: Vec<ExternalSpecImplementationMetadata>,
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

/// A session-local handoff. Persist facts/artifacts through their own codecs;
/// source-analysis relationships must not become portable proof by deserialization.
#[derive(Debug)]
pub struct EffectPipelineOutput {
    pub artifacts: EffectPipelineArtifacts,
    pub effects: EffectOutput,
    pub trace_specs: TraceSpecPipelineAnalysis,
}

/// Original session-local analysis and classification from this pipeline run.
/// These are inputs to plan lowering, not executable plans or installation proof.
pub struct TraceSpecPipelineAnalysis {
    pub(super) analysis: crate::trace_spec::TraceSpecAnalysisOutput,
    pub(super) dispositions: crate::trace_spec::TraceStaticDispositions,
    pub(super) residual_plans: crate::residual::VerifiedResidualPlans,
}

impl std::fmt::Debug for TraceSpecPipelineAnalysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceSpecPipelineAnalysis")
            .field(
                "source_conformances",
                &self.analysis.source_conformances.len(),
            )
            .field("residual_plans", &self.residual_plans)
            .finish_non_exhaustive()
    }
}

impl TraceSpecPipelineAnalysis {
    pub fn analysis(&self) -> &crate::trace_spec::TraceSpecAnalysisOutput {
        &self.analysis
    }

    pub fn dispositions(&self) -> &crate::trace_spec::TraceStaticDispositions {
        &self.dispositions
    }

    /// Plans verified against this run's analysis. Backend support, final
    /// binding and installation are separate gates.
    pub fn residual_plans(&self) -> &crate::residual::VerifiedResidualPlans {
        &self.residual_plans
    }

    /// Releases the verified plans; the analysis inputs are dropped.
    pub fn into_residual_plans(self) -> crate::residual::VerifiedResidualPlans {
        self.residual_plans
    }

    pub fn into_parts(
        self,
    ) -> (
        crate::trace_spec::TraceSpecAnalysisOutput,
        crate::trace_spec::TraceStaticDispositions,
    ) {
        (self.analysis, self.dispositions)
    }
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
        validate_source_snapshot_identities(input.source_snapshots, control.as_deref())?;
        for entry in input.external_summaries {
            if entry.returned_callables.as_ref().is_some_and(|graph| {
                graph
                    .values
                    .iter()
                    .any(|value| !value.residual_checks.is_empty())
            }) {
                return Err(EffectPipelineError::InvalidExternalMetadata {
                    package: entry
                        .metadata
                        .package
                        .clone()
                        .unwrap_or_else(|| entry.package.to_string()),
                    reason:
                        "external callable residual references require validated residual plans"
                            .into(),
                });
            }
        }
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
    use crate::pipeline::artifacts::{
        EFFECT_REGISTRY, FROZEN_CALL_SUMMARIES, MEMORY_PROVENANCE, TRACE_SPEC_ANALYSIS,
    };
    use crate::pipeline::context::EffectPipelineContext;
    use crate::pipeline::passes::{
        AnalyzeMemoryProvenancePass, BindFrozenCallsPass, BuildRegistryPass, ClassifyTraceSpecPass,
        CollectUnitsPass, SolveSummariesPass,
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
    fn source_snapshot_identity_validation_obeys_limits_and_cancellation() {
        struct Stop;
        impl etas_utils::PipelineExecutionControl for Stop {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancelled source identity validation".into(),
                })
            }
        }
        let key = etas_code::StableSourceKey::new(
            etas_code::StableSourceOrigin::Project(etas_code::CodeFingerprint::from_bytes([1; 32])),
            &["main.es".into()],
        )
        .unwrap();
        let snapshot = EffectSourceSnapshot {
            source: etas_core::SourceId(1),
            key,
            text: "",
        };
        assert!(matches!(
            super::validate_source_snapshot_identities(&[snapshot], Some(&Stop)),
            Err(EffectPipelineError::Interrupted(_))
        ));
        let too_many = vec![snapshot; etas_package_metadata::MAX_METADATA_GRAPH_NODES + 1];
        assert!(matches!(
            super::validate_source_snapshot_identities(&too_many, None),
            Err(EffectPipelineError::AnalysisLimitExceeded {
                resource: "effect source snapshots",
                ..
            })
        ));
    }

    #[test]
    fn duplicate_source_snapshot_identities_are_rejected_before_analysis() {
        use etas_code::{CodeFingerprint, StableSourceKey, StableSourceOrigin};
        let key = |byte| {
            StableSourceKey::new(
                StableSourceOrigin::Project(CodeFingerprint::from_bytes([byte; 32])),
                &["main.es".into()],
            )
            .unwrap()
        };
        let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
        let types = TypeOutput::default();
        let registry = etas_std::standard_registry();
        for snapshots in [
            vec![
                EffectSourceSnapshot {
                    source: etas_core::SourceId(1),
                    key: key(1),
                    text: "a",
                },
                EffectSourceSnapshot {
                    source: etas_core::SourceId(1),
                    key: key(2),
                    text: "b",
                },
            ],
            vec![
                EffectSourceSnapshot {
                    source: etas_core::SourceId(1),
                    key: key(1),
                    text: "a",
                },
                EffectSourceSnapshot {
                    source: etas_core::SourceId(2),
                    key: key(1),
                    text: "a",
                },
            ],
        ] {
            let input = EffectPipelineInput {
                external_implementations: &[],
                source_snapshots: &snapshots,
                residual_identity: None,
                trace_monitor_projector: None,
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
            assert!(matches!(
                RunEffectPipeline::run(input),
                Err(EffectPipelineError::InvalidSourceIdentity { .. })
            ));
        }
    }

    #[test]
    fn direct_pipeline_rejects_external_residual_without_validated_plan() {
        use etas_code::{
            CodeFingerprint,
            residual::identity::{ObligationId, ResidualObligationRef},
        };
        use etas_package_metadata::{
            ActionTrace, CallableValueBehavior, CallableValueDeterminism, CallableValueGraph,
            CallableValueNode, CallableValueScope, CallableValueSource, EffectRow, Type,
        };
        let reference = ResidualObligationRef::new(
            ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([3; 32])),
            Default::default(),
        )
        .unwrap();
        let graph = CallableValueGraph {
            objects: Vec::new(),
            sources: vec![CallableValueSource::Value(0)],
            values: vec![CallableValueNode {
                residual_checks: vec![reference],
                handler_requests: Some(Default::default()),
                behavior: CallableValueBehavior {
                    determinism: CallableValueDeterminism::Deterministic,
                    independent_determinism: Some(CallableValueDeterminism::Deterministic),
                    host_requirements: vec![],
                    interpreter_features: vec![],
                },
                parent_scope: CallableValueScope::Root,
                generic_params: vec![],
                generic_bindings: vec![],
                param_names: vec![],
                params: vec![],
                output: Type {
                    name: "unit".into(),
                    ..Default::default()
                },
                public_effects: EffectRow::default(),
                requested_actions: EffectRow::default(),
                handled_requested_actions: EffectRow::default(),
                default_handler_effects: Default::default(),
                parameter_bindings: vec![],
                action_trace: ActionTrace::Empty,
                returned_sources: None,
            }],
        };
        let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
        let types = TypeOutput::default();
        let registry = etas_std::standard_registry();
        let summaries = [AnchoredExternalMetadata {
            package: 17,
            metadata: ExternalEffectSummaryMetadata {
                package: Some("dependency".into()),
                returned_callables: Some(graph),
                ..Default::default()
            },
            span: etas_core::Span::new(
                etas_core::SourceId(0),
                etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(1)),
            ),
        }];
        let input = EffectPipelineInput {
            external_implementations: &[],
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
            frozen_calls: None,
            hir: &hir,
            types: &types,
            std_registry: &registry,
            dependency_metadata: None,
            tool_bindings: &[],
            external_summaries: &summaries,
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        };
        assert!(matches!(
            RunEffectPipeline::run(input),
            Err(EffectPipelineError::InvalidExternalMetadata { package, reason })
                if package == "dependency" && reason.contains("validated residual plans")
        ));
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
                source_snapshots: &[],
                residual_identity: None,
                trace_monitor_projector: None,
                external_implementations: &[],
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

    #[test]
    fn trace_disposition_pass_requires_actual_monitor_analysis() {
        let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
        let types = TypeOutput::default();
        let registry = etas_std::standard_registry();
        let input = EffectPipelineInput {
            external_implementations: &[],
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
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
        let mut pipeline =
            Pipeline::new("effects.missing_trace_analysis").pass(ClassifyTraceSpecPass);
        let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);
        let PassControl::Failed(failure) = result.control else {
            panic!("trace classification accepted missing analysis");
        };
        assert_eq!(failure.missing_artifact, Some(TRACE_SPEC_ANALYSIS));
        assert!(context.trace_static_dispositions.is_none());
    }

    #[test]
    fn residual_plan_stages_cannot_be_omitted_or_lost_before_validation_or_emit() {
        use crate::pipeline::passes::{
            AnalyzeTraceSpecMonitorsPass, EmitOutputPass, LowerResidualMonitorPlansPass,
            MaterializeFactsPass, MaterializeTraceSpecModelsPass, ValidateContractsPass,
            ValidateTraceSpecPass, VerifyResidualPlansPass,
        };
        use etas_utils::{ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassResult};

        struct LoseVerifiedPlans;
        impl<'a> Pass<EffectPipelineContext<'a>> for LoseVerifiedPlans {
            fn descriptor(&self) -> PassDescriptor {
                PassDescriptor::new("tests.lose_verified_plans", PassKind::Transform).requires(
                    ArtifactSet::one(crate::pipeline::artifacts::VERIFIED_RESIDUAL_PLANS),
                )
            }
            fn run(
                &mut self,
                context: &mut EffectPipelineContext<'a>,
                _: &PassContext<EffectPipelineContext<'a>>,
                _: &mut PassManager<EffectPipelineContext<'a>>,
            ) -> PassResult {
                context.verified_residual_plans = None;
                PassResult::unchanged()
            }
        }

        let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
        let types = TypeOutput::default();
        let registry = etas_std::standard_registry();
        let input = EffectPipelineInput {
            external_implementations: &[],
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
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
        let prefix = || {
            Pipeline::new("effects.residual_stages")
                .pass(BuildRegistryPass)
                .pass(BindFrozenCallsPass)
                .pass(CollectUnitsPass)
                .pass(AnalyzeMemoryProvenancePass)
                .pass(SolveSummariesPass)
                .pass(MaterializeFactsPass)
                .pass(ValidateContractsPass)
                .pass(MaterializeTraceSpecModelsPass)
                .pass(AnalyzeTraceSpecMonitorsPass)
                .pass(ClassifyTraceSpecPass)
        };

        // Verification cannot run on a lowering result that was never produced.
        let mut context = EffectPipelineContext::new(input);
        let mut pipeline = prefix().pass(VerifyResidualPlansPass);
        let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);
        let PassControl::Failed(failure) = result.control else {
            panic!("verification accepted missing lowered plans");
        };
        assert_eq!(
            failure.missing_artifact,
            Some(crate::pipeline::artifacts::RESIDUAL_MONITOR_PLANS)
        );
        assert!(context.verified_residual_plans.is_none());

        // Trace validation requires verified plans, not merely lowered ones.
        let mut context = EffectPipelineContext::new(input);
        let mut pipeline = prefix()
            .pass(LowerResidualMonitorPlansPass)
            .pass(ValidateTraceSpecPass);
        let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);
        let PassControl::Failed(failure) = result.control else {
            panic!("trace validation accepted unverified residual plans");
        };
        assert_eq!(
            failure.missing_artifact,
            Some(crate::pipeline::artifacts::VERIFIED_RESIDUAL_PLANS)
        );

        // Verification consumes the lowered value; a lost verified value is
        // rejected by emission even though the artifact marker exists.
        let mut context = EffectPipelineContext::new(input);
        let mut pipeline = prefix()
            .pass(LowerResidualMonitorPlansPass)
            .pass(VerifyResidualPlansPass)
            .pass(ValidateTraceSpecPass)
            .pass(LoseVerifiedPlans)
            .pass(EmitOutputPass);
        let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);
        assert!(matches!(result.control, PassControl::Failed(_)));
        assert!(context.residual_monitor_plans.is_none());
        assert_eq!(
            context.failure,
            Some(EffectPipelineError::MissingStageArtifact {
                stage: crate::EffectStage::Emit,
                artifact: "verified residual plans",
            })
        );
        assert!(context.artifacts.is_none());
    }

    #[test]
    fn emit_rejects_lost_trace_analysis_even_when_artifact_markers_exist() {
        use crate::pipeline::passes::{
            AnalyzeTraceSpecMonitorsPass, EmitOutputPass, LowerResidualMonitorPlansPass,
            MaterializeFactsPass, MaterializeTraceSpecModelsPass, ValidateContractsPass,
            ValidateTraceSpecPass, VerifyResidualPlansPass,
        };
        use etas_utils::{ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassResult};

        struct LoseTraceArtifact(bool);
        impl<'a> Pass<EffectPipelineContext<'a>> for LoseTraceArtifact {
            fn descriptor(&self) -> PassDescriptor {
                PassDescriptor::new("tests.lose_trace_artifact", PassKind::Transform).requires(
                    ArtifactSet::from([
                        crate::pipeline::artifacts::TRACE_STATIC_DISPOSITIONS,
                        crate::pipeline::artifacts::VALIDATED_TRACE_SPEC_FACTS,
                    ]),
                )
            }
            fn run(
                &mut self,
                context: &mut EffectPipelineContext<'a>,
                _: &PassContext<EffectPipelineContext<'a>>,
                _: &mut PassManager<EffectPipelineContext<'a>>,
            ) -> PassResult {
                if self.0 {
                    context.trace_spec_analysis = None;
                } else {
                    context.trace_static_dispositions = None;
                }
                PassResult::unchanged()
            }
        }

        for lose_analysis in [true, false] {
            let hir = etas_hir::HirRevision::try_new(HirProgram::default()).unwrap();
            let types = TypeOutput::default();
            let registry = etas_std::standard_registry();
            let input = EffectPipelineInput {
                external_implementations: &[],
                source_snapshots: &[],
                residual_identity: None,
                trace_monitor_projector: None,
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
            let mut incomplete = Pipeline::new("effects.reject_missing_trace_stage")
                .pass(BuildRegistryPass)
                .pass(BindFrozenCallsPass)
                .pass(CollectUnitsPass)
                .pass(AnalyzeMemoryProvenancePass)
                .pass(SolveSummariesPass)
                .pass(MaterializeFactsPass)
                .pass(ValidateContractsPass)
                .pass(MaterializeTraceSpecModelsPass);
            if !lose_analysis {
                incomplete = incomplete.pass(AnalyzeTraceSpecMonitorsPass);
            }
            incomplete = incomplete.pass(EmitOutputPass);
            let result = PassManager::new().run_global_pipeline(&mut incomplete, &mut context);
            let PassControl::Failed(failure) = result.control else {
                panic!("emission accepted missing trace stage artifacts");
            };
            assert_eq!(
                failure.missing_artifact,
                Some(if lose_analysis {
                    crate::pipeline::artifacts::TRACE_SPEC_ANALYSIS
                } else {
                    crate::pipeline::artifacts::TRACE_STATIC_DISPOSITIONS
                })
            );
            assert!(context.artifacts.is_none());

            let mut context = EffectPipelineContext::new(input);
            let mut pipeline = Pipeline::new("effects.reject_lost_trace_artifact")
                .pass(BuildRegistryPass)
                .pass(BindFrozenCallsPass)
                .pass(CollectUnitsPass)
                .pass(AnalyzeMemoryProvenancePass)
                .pass(SolveSummariesPass)
                .pass(MaterializeFactsPass)
                .pass(ValidateContractsPass)
                .pass(MaterializeTraceSpecModelsPass)
                .pass(AnalyzeTraceSpecMonitorsPass)
                .pass(ClassifyTraceSpecPass)
                .pass(LowerResidualMonitorPlansPass)
                .pass(VerifyResidualPlansPass)
                .pass(ValidateTraceSpecPass)
                .pass(LoseTraceArtifact(lose_analysis))
                .pass(EmitOutputPass);
            let result = PassManager::new().run_global_pipeline(&mut pipeline, &mut context);
            assert!(matches!(result.control, PassControl::Failed(_)));
            assert_eq!(
                context.failure,
                Some(EffectPipelineError::MissingStageArtifact {
                    stage: crate::EffectStage::Emit,
                    artifact: if lose_analysis {
                        "trace spec analysis"
                    } else {
                        "trace static dispositions"
                    },
                })
            );
            assert!(context.artifacts.is_none());
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
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
            external_implementations: &[],
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
