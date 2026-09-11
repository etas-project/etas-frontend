use std::collections::BTreeSet;

use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_hir::HirItemId;
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, Pipeline,
    PreservedArtifacts,
};

use crate::infer::analysis::{EffectAnalysisInput, run_effect_analysis};
use crate::infer::unit::collect::EffectUnitCollector;
use crate::{
    DependencyEffectMetadata, EffectPipelineError, EffectRegistry, EffectStage, EffectUnit,
};

use super::artifacts::{
    EFFECT_FACTS, EFFECT_OUTPUT, EFFECT_REGISTRY, EFFECT_SUMMARIES, EFFECT_UNITS,
    MEMORY_PROVENANCE, TRACE_SPEC_ANALYSIS, TRACE_SPEC_MODELS, VALIDATED_EFFECT_FACTS,
    VALIDATED_TRACE_SPEC_FACTS,
};
use super::context::EffectPipelineContext;
use super::{EffectPipelineArtifacts, ExternalArtifactAnchor};

mod build_registry;
pub(super) use build_registry::BuildRegistryPass;
mod collect_units;
pub(super) use collect_units::CollectUnitsPass;
mod analyze_memory_provenance;
mod solve_summaries;
pub(super) use analyze_memory_provenance::AnalyzeMemoryProvenancePass;
pub(super) use solve_summaries::SolveSummariesPass;
mod materialize_facts;
pub(super) use materialize_facts::MaterializeFactsPass;
mod validate_contracts;
pub(super) use validate_contracts::ValidateContractsPass;
mod materialize_trace_spec_models;
pub(super) use materialize_trace_spec_models::MaterializeTraceSpecModelsPass;
mod analyze_trace_spec_monitors;
pub(super) use analyze_trace_spec_monitors::AnalyzeTraceSpecMonitorsPass;
mod validate_trace_spec;
pub(super) use validate_trace_spec::ValidateTraceSpecPass;
mod emit_output;
pub(super) use emit_output::EmitOutputPass;

pub(super) fn build_effect_pipeline<'a>() -> Pipeline<EffectPipelineContext<'a>> {
    Pipeline::new("effects.run")
        .pass(BuildRegistryPass)
        .pass(CollectUnitsPass)
        .pass(AnalyzeMemoryProvenancePass)
        .pass(SolveSummariesPass)
        .pass(MaterializeFactsPass)
        .pass(ValidateContractsPass)
        .pass(MaterializeTraceSpecModelsPass)
        .pass(AnalyzeTraceSpecMonitorsPass)
        .pass(ValidateTraceSpecPass)
        .pass(EmitOutputPass)
}

fn produced(key: etas_utils::ArtifactKey) -> PassResult {
    PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(key))
}

fn missing(stage: EffectStage, artifact: &'static str) -> EffectPipelineError {
    EffectPipelineError::MissingStageArtifact { stage, artifact }
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
