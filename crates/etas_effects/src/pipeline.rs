use std::collections::BTreeSet;

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::HirItemId;
use etas_hir_analysis::HirAnalysisContext;
use etas_types::EffectArgRef;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassControl, PassDescriptor, PassFailure, PassKind,
    PassManager, PassResult, Pipeline, PipelineConfig,
};

use crate::infer::analysis::{EffectAnalysisInput, run_effect_analysis};
use crate::infer::unit::collect::EffectUnitCollector;
use crate::{
    DependencyEffectMetadata, EffectAnalysisOutput, EffectOutput, EffectPipelineError,
    EffectRegistry, EffectStage, EffectUnit, ToolProviderBindingMetadata,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalEffectSummaryMetadata {
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub import_root: Option<String>,
    pub item: Vec<String>,
    #[serde(default)]
    pub param_names: Vec<String>,
    pub public_effects: ExternalEffectRowMetadata,
    pub requested_actions: ExternalEffectRowMetadata,
    #[serde(default)]
    pub handled_requested_actions: ExternalEffectRowMetadata,
    #[serde(default)]
    pub latent_flows: Vec<ExternalLatentFlowSummaryMetadata>,
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

struct BuildRegistryPass;
struct CollectUnitsPass;
struct SolveSummariesPass;
struct MaterializeFactsPass;
struct ValidateContractsPass;
struct MaterializeTraceSpecModelsPass;
struct AnalyzeTraceSpecMonitorsPass;
struct ValidateTraceSpecPass;
struct EmitOutputPass;

struct EffectPipelineContext<'a> {
    input: EffectPipelineInput<'a>,
    analysis_context: HirAnalysisContext,
    registry: Option<EffectRegistry>,
    units: Option<Vec<EffectUnit>>,
    registry_diagnostics: Vec<Diagnostic>,
    artifacts: Option<EffectPipelineArtifacts>,
    analysis: Option<EffectAnalysisOutput>,
    effects: Option<EffectOutput>,
    trace_spec_models: Option<crate::trace_spec::TraceSpecModelStore>,
    trace_spec_analysis: Option<crate::trace_spec::TraceSpecAnalysisOutput>,
    failure: Option<EffectPipelineError>,
}

impl<'a> EffectPipelineContext<'a> {
    fn new(input: EffectPipelineInput<'a>) -> Self {
        Self {
            analysis_context: HirAnalysisContext::new(input.hir),
            input,
            registry: None,
            units: None,
            registry_diagnostics: Vec::new(),
            artifacts: None,
            analysis: None,
            effects: None,
            trace_spec_models: None,
            trace_spec_analysis: None,
            failure: None,
        }
    }

    fn into_output(self) -> Result<EffectPipelineOutput, EffectPipelineError> {
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

    fn fail(&mut self, error: EffectPipelineError) -> PassResult {
        let message = error.to_string();
        self.failure = Some(error);
        PassResult::failed(message)
    }
}

impl From<EffectPipelineArtifacts> for EffectInferencePlan {
    fn from(artifacts: EffectPipelineArtifacts) -> Self {
        Self {
            registry: artifacts.registry,
        }
    }
}

fn effect_pipeline<'a>() -> Pipeline<EffectPipelineContext<'a>> {
    Pipeline::new("effects.run")
        .pass(BuildRegistryPass)
        .pass(CollectUnitsPass)
        .pass(SolveSummariesPass)
        .pass(MaterializeFactsPass)
        .pass(ValidateContractsPass)
        .pass(MaterializeTraceSpecModelsPass)
        .pass(AnalyzeTraceSpecMonitorsPass)
        .pass(ValidateTraceSpecPass)
        .pass(EmitOutputPass)
}

impl<'a> Pass<EffectPipelineContext<'a>> for BuildRegistryPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.build_registry", PassKind::Analysis)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let empty_dependency_metadata = DependencyEffectMetadata::default();
        let registry = match EffectRegistry::from_hir_types_and_dependencies(
            context.input.hir,
            context.input.types,
            context
                .input
                .dependency_metadata
                .unwrap_or(&empty_dependency_metadata),
        ) {
            Ok(registry) => registry,
            Err(error) => return context.fail(error),
        };
        match dependency_registry_diagnostics(&registry, context.input.external_artifact_anchors) {
            Ok(diagnostics) => context.registry_diagnostics = diagnostics,
            Err(error) => return context.fail(error),
        }
        context.registry = Some(registry);
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for CollectUnitsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.collect_units", PassKind::Analysis)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let mut units =
            EffectUnitCollector::collect_with_context(context.input.hir, &context.analysis_context);
        if let Some(reachable_items) = context.input.reachable_items {
            units.retain(|unit| effect_unit_owner_is_reachable(unit, reachable_items));
        }
        context.units = Some(units);
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

fn effect_unit_owner_is_reachable(
    unit: &EffectUnit,
    reachable_items: &BTreeSet<HirItemId>,
) -> bool {
    match unit {
        EffectUnit::Item(item) => reachable_items.contains(item),
        EffectUnit::HandlerArm { owner, .. }
        | EffectUnit::AnonymousFlow { owner, .. }
        | EffectUnit::FirstClassFlowCall { owner, .. } => reachable_items.contains(owner),
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for SolveSummariesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.solve_summaries", PassKind::Analysis)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::SolveSummaries,
                artifact: "effect registry",
            });
        };
        let Some(units) = context.units.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::SolveSummaries,
                artifact: "effect units",
            });
        };
        let analysis = match run_effect_analysis(
            EffectAnalysisInput {
                hir: context.input.hir,
                types: context.input.types,
                registry,
                tool_bindings: context.input.tool_bindings,
                external_summaries: context.input.external_summaries,
            },
            units,
            context.analysis_context.clone(),
        ) {
            Ok(analysis) => analysis,
            Err(error) => return context.fail(error),
        };
        context.analysis = Some(analysis);
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for ValidateContractsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.validate_contracts", PassKind::Verify)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::ValidateContracts,
                artifact: "effect registry",
            });
        };
        let Some(mut staged_effects) = context.effects.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::ValidateContracts,
                artifact: "materialized effect facts",
            });
        };
        if let Err(error) = crate::validate::EffectContractValidator::validate(
            context.input.hir,
            context.input.types,
            &registry,
            context.input.tool_bindings,
            &mut staged_effects,
            context.input.reachable_items,
        ) {
            return context.fail(error);
        }
        context.effects = Some(staged_effects);
        PassResult::unchanged()
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for MaterializeTraceSpecModelsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.materialize_models", PassKind::Analysis)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::MaterializeTraceSpecs,
                artifact: "effect registry",
            });
        };
        let Some(mut staged_effects) = context.effects.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::MaterializeTraceSpecs,
                artifact: "effect facts",
            });
        };
        let models = match crate::trace_spec::materialize_trace_spec_models(
            context.input.hir,
            context.input.types,
            &registry,
            context.input.external_trace_specs,
            &mut staged_effects,
        ) {
            Ok(models) => models,
            Err(error) => return context.fail(error),
        };
        context.effects = Some(staged_effects);
        context.trace_spec_models = Some(models);
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for AnalyzeTraceSpecMonitorsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.analyze_monitors", PassKind::Analysis)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::AnalyzeTraceSpecs,
                artifact: "effect registry",
            });
        };
        let Some(models) = context.trace_spec_models.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::AnalyzeTraceSpecs,
                artifact: "trace spec models",
            });
        };
        let Some(mut staged_effects) = context.effects.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::AnalyzeTraceSpecs,
                artifact: "effect facts",
            });
        };
        let Some(units) = context.units.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::AnalyzeTraceSpecs,
                artifact: "effect units",
            });
        };
        let analysis = match crate::trace_spec::analyze_trace_spec_monitors(
            context.input.hir,
            context.input.types,
            &registry,
            &mut staged_effects,
            &models,
            units,
            context.analysis_context.clone(),
        ) {
            Ok(analysis) => analysis,
            Err(error) => return context.fail(error),
        };
        context.effects = Some(staged_effects);
        context.trace_spec_analysis = Some(analysis);
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for ValidateTraceSpecPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.validate", PassKind::Verify)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::ValidateTraceSpecs,
                artifact: "effect registry",
            });
        };
        let Some(models) = context.trace_spec_models.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::ValidateTraceSpecs,
                artifact: "trace spec models",
            });
        };
        let Some(analysis) = context.trace_spec_analysis.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::ValidateTraceSpecs,
                artifact: "trace spec analysis",
            });
        };
        let Some(mut staged_effects) = context.effects.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::ValidateTraceSpecs,
                artifact: "effect facts",
            });
        };
        if let Err(error) = crate::trace_spec::validate_trace_spec_facts(
            context.input.hir,
            context.input.types,
            &registry,
            &mut staged_effects,
            &models,
            &analysis,
            context.input.external_summaries,
            context.input.reachable_items,
        ) {
            return context.fail(error);
        }
        context.effects = Some(staged_effects);
        PassResult::unchanged()
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for MaterializeFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.materialize_facts", PassKind::Analysis)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::MaterializeFacts,
                artifact: "effect registry",
            });
        };
        let Some(analysis) = context.analysis.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::MaterializeFacts,
                artifact: "solved effect summaries",
            });
        };
        let effects = crate::facts::EffectFactProjector::materialize(
            context.input.hir,
            context.input.types,
            registry,
            analysis,
            context.input.reachable_items,
        );
        match effects {
            Ok(effects) => context.effects = Some(effects),
            Err(error) => return context.fail(error),
        }
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for EmitOutputPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.emit_output", PassKind::Emit)
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::Emit,
                artifact: "effect registry",
            });
        };
        let Some(units) = context.units.clone() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::Emit,
                artifact: "effect units",
            });
        };
        let Some(mut effects) = context.effects.take() else {
            return context.fail(EffectPipelineError::MissingStageArtifact {
                stage: EffectStage::Emit,
                artifact: "validated effect output",
            });
        };
        let artifacts = EffectPipelineArtifacts {
            registry,
            dependency_metadata: context
                .input
                .dependency_metadata
                .cloned()
                .unwrap_or_default(),
            tool_bindings: context.input.tool_bindings.to_vec(),
            external_summaries: context
                .input
                .external_summaries
                .iter()
                .map(|summary| summary.metadata.clone())
                .collect(),
            external_trace_specs: context
                .input
                .external_trace_specs
                .iter()
                .map(|summary| summary.metadata.clone())
                .collect(),
            units,
            pass_timings: Vec::new(),
        };
        effects
            .diagnostics
            .extend(context.registry_diagnostics.clone());
        context.effects = Some(effects);
        context.artifacts = Some(artifacts);
        PassResult::changed(etas_utils::PreservedArtifacts::All, ArtifactSet::new())
    }
}

fn dependency_registry_diagnostics(
    registry: &EffectRegistry,
    anchors: &[ExternalArtifactAnchor],
) -> Result<Vec<Diagnostic>, EffectPipelineError> {
    registry
        .unresolved_dependency_extensions()
        .iter()
        .map(|extension| {
            let mut matches = anchors.iter().filter(|anchor| {
                anchor.item == extension.child
                    && extension
                        .package
                        .is_none_or(|package| anchor.package == package)
            });
            let anchor =
                matches
                    .next()
                    .ok_or_else(|| EffectPipelineError::MissingDiagnosticAnchor {
                        artifact: effect_path_label(&extension.child),
                    })?;
            if let Some(other) = matches.next() {
                return Err(EffectPipelineError::InvalidExternalMetadata {
                    package: format!("{} / {}", anchor.package_label, other.package_label),
                    reason: format!(
                        "effect extension `{}` has ambiguous source anchors",
                        effect_path_label(&extension.child)
                    ),
                });
            }
            Ok(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                anchor.span,
                format!(
                    "dependency effect extension could not be resolved: {} extends {}",
                    effect_path_label(&extension.child),
                    effect_path_label(&extension.parent)
                ),
            ))
        })
        .collect()
}

fn effect_path_label(path: &[String]) -> String {
    if path.is_empty() {
        "<empty>".to_owned()
    } else {
        path.join(".")
    }
}

#[cfg(test)]
mod tests {
    use etas_utils::ArtifactKey;

    use super::*;

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
}
