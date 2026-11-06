use etas_utils::{Pipeline, UnitOrder, UnitSelector};

use crate::passes::artifacts::TYPE_OUTPUT;
use crate::passes::{
    AdmitStandardLibrarySourcesPass, AgentEffectLegalityPass, AnalyzeConcurrencyPass,
    AnalyzeLoopProgressPass,
    ApplyQualityContextBudgetPolicyPass, ApplyResolvedImportsToHirPass, BatchStorageSemijoinPass,
    BindMethodOriginsPass, BuildCheckedProjectPass, BuildImportGraphPass, BuildModuleCatalogPass,
    BuildModuleIndexPass, BuildSignatureFactsPass, BuildSourceSetPass, BuildUnitTreePass,
    CheckStandardLibraryImplementationsPass, CompleteEffectRuntimeReachabilityPass,
    CompleteTypedAnalysisReachabilityPass, CompleteTypedRuntimeReachabilityPass,
    ComputeAffectedModulesPass, ComputeDefinitionAnalysisReachabilityPass,
    ComputeEntryReachabilityPass, ComputeModuleTopoOrderPass, DetectImportCyclesPass,
    FinalizeProjectHirPass, FinalizeTypeFactsPass, FuseProjectionScansPass,
    FuseSerializationBoundaryPass, InferCallableOutputsPass, LowerModuleItemsPass,
    LowerOwnedSnapshotCowPass, LowerResourceRegionPass, MaterializeGenericEffectRowsPass,
    NormalizeModuleImportsPass, ParseSourceFilePass, PredeclareProjectSymbolsPass,
    PrepareBodyTypeEnvironmentPass, PrepareDirectInvocationTypesPass, PrepareEffectRegistryPass,
    PrepareInvocationTypesPass, PrepareVersionedRequestPass, ProjectAgentObservationPass,
    PruneSupersededAgentEventsPass, PruneUnusedQualityContextPass, ResolveEntryItemPass,
    ResolveImportTargetsPass, ResolvePathsPass, RetainDefinitionSourcesPass,
    ReuseTypeBodyFactsPass, RunEffectPipelinePass, ScheduleBoundedPipelinePass,
    ShortCircuitAgentThresholdPass, SpecializeAgentToolSchemasPass,
    SpecializeDeterministicDispatchPass, SuffixDemandHistoryPass, SynthesizeScopedIndexPass,
    TypeCheckBodyPass, ValidateEntryContractPass, ValidateExternalEnvironmentPass,
    ValidateHandlerInstantiationsPass, ValidateItemAnnotationsPass,
    ValidateResidualBackendSupportPass, ValidateTopLevelLetPass, VerifyInterpreterSupportPass,
    VersionedPreparationCsePass,
};
use crate::{
    BODY_UNIT_KIND, MODULE_PART_UNIT_KIND, ProjectContext, SOURCE_FILE_UNIT_KIND,
    project::ANALYZED_BODY_UNIT_FILTER,
};

pub fn project_pipeline(plan: &crate::optimization::OptimizationPlan) -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.check_project")
        .group(
            Pipeline::new("frontend.project_sources")
                .pass(BuildSourceSetPass)
                .for_each(
                    UnitSelector::Kind(SOURCE_FILE_UNIT_KIND),
                    UnitOrder::SourceOrder,
                    Pipeline::new("frontend.source").pass(ParseSourceFilePass),
                )
                .pass(AdmitStandardLibrarySourcesPass)
                .for_each(
                    UnitSelector::Kind(SOURCE_FILE_UNIT_KIND),
                    UnitOrder::SourceOrder,
                    Pipeline::new("frontend.standard_library_source").pass(ParseSourceFilePass),
                ),
        )
        .group(semantic_pipeline(false, plan))
        .pass(BuildCheckedProjectPass)
}

pub(crate) fn candidate_body_pipeline(
    plan: &crate::optimization::OptimizationPlan,
) -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.check_candidate_body")
        .pass(crate::code::pipeline::passes::FreezeDefinitionEnvironmentPass)
        .pass(crate::code::pipeline::passes::ParseCandidateDependencySourcesPass)
        .group(semantic_pipeline(true, plan))
        .pass(BuildCheckedProjectPass)
        .pass(crate::code::pipeline::passes::MaterializeCandidateInvocationTypesPass)
        .pass(crate::code::pipeline::passes::BuildDefinitionDescriptorsPass)
        .pass(crate::code::pipeline::passes::ValidateCandidateContractPass)
        .pass(crate::code::pipeline::passes::EmitCheckedCandidatePass)
}

fn semantic_pipeline(
    candidate: bool,
    plan: &crate::optimization::OptimizationPlan,
) -> Pipeline<ProjectContext> {
    let mut pipeline = Pipeline::new("frontend.project_semantics")
        .pass(BuildModuleIndexPass)
        .pass(BuildModuleCatalogPass)
        .pass(ResolveImportTargetsPass)
        .pass(ValidateExternalEnvironmentPass)
        .pass(crate::passes::BindValidatedExternalImportTargetsPass)
        .pass(BuildUnitTreePass)
        .pass(BuildImportGraphPass)
        .pass(DetectImportCyclesPass)
        .pass(ComputeModuleTopoOrderPass)
        .pass(ComputeAffectedModulesPass)
        .pass(PredeclareProjectSymbolsPass);
    if candidate {
        pipeline.push_pass(crate::code::pipeline::passes::ApplyCandidateBodyOverlaysPass);
    }
    pipeline = pipeline
        .for_each(
            UnitSelector::Kind(MODULE_PART_UNIT_KIND),
            UnitOrder::DependencyOrder,
            Pipeline::new("frontend.module_part")
                .pass(NormalizeModuleImportsPass)
                .pass(LowerModuleItemsPass),
        )
        .pass(FinalizeProjectHirPass)
        .pass(ApplyResolvedImportsToHirPass)
        .pass(ResolvePathsPass);
    if candidate {
        pipeline.push_pass(crate::code::pipeline::passes::ResolveCandidateTargetPass);
    } else {
        pipeline.push_pass(ResolveEntryItemPass);
    }
    pipeline = pipeline
        .pass(ComputeEntryReachabilityPass)
        .pass(BuildSignatureFactsPass)
        .pass(ValidateTopLevelLetPass)
        .pass(ValidateItemAnnotationsPass)
        .pass(InferCallableOutputsPass)
        .pass(RetainDefinitionSourcesPass)
        .pass(ComputeDefinitionAnalysisReachabilityPass)
        .pass(PrepareBodyTypeEnvironmentPass)
        .pass(PrepareEffectRegistryPass)
        .pass(BindMethodOriginsPass);
    if candidate {
        pipeline.push_pass(crate::code::pipeline::passes::PrepareCandidateApplicationPass);
    }
    pipeline = pipeline
        .pass(ReuseTypeBodyFactsPass)
        .for_each(
            UnitSelector::AffectedArtifact {
                kind: BODY_UNIT_KIND,
                artifact: TYPE_OUTPUT,
                filter: Some(ANALYZED_BODY_UNIT_FILTER),
            },
            UnitOrder::Stable,
            Pipeline::new("frontend.type_body").pass(TypeCheckBodyPass),
        )
        .pass(FinalizeTypeFactsPass)
        .pass(CompleteTypedAnalysisReachabilityPass)
        .pass(CompleteTypedRuntimeReachabilityPass)
        .pass(PrepareDirectInvocationTypesPass)
        .pass(RunEffectPipelinePass)
        .pass(AnalyzeConcurrencyPass)
        .pass(ValidateResidualBackendSupportPass)
        .pass(CompleteEffectRuntimeReachabilityPass)
        .pass(MaterializeGenericEffectRowsPass)
        .pass(CheckStandardLibraryImplementationsPass)
        .pass(ValidateHandlerInstantiationsPass)
        .pass(AnalyzeLoopProgressPass)
        .group(optimization_pipeline(plan))
        .pass(VerifyInterpreterSupportPass);
    if !candidate {
        pipeline.push_pass(ValidateEntryContractPass);
    }
    pipeline
        .pass(PrepareInvocationTypesPass)
        .pass(crate::code::pipeline::passes::ResolveReflectionTargetsPass)
}

fn optimization_pipeline(plan: &crate::optimization::OptimizationPlan) -> Pipeline<ProjectContext> {
    let mut pipeline = Pipeline::new("frontend.hir_optimization");
    let needs_legality = plan.level.enables(crate::OptimizationLevel::O1)
        || plan.quality_context_token_budget.is_some();
    if needs_legality {
        pipeline.push_pass(AgentEffectLegalityPass);
    }
    if plan.level.enables(crate::OptimizationLevel::O1) {
        let mut body_pipeline = Pipeline::new("frontend.hir_optimization.body");
        body_pipeline.push_group(o1_pipeline());
        if plan.level.enables(crate::OptimizationLevel::O2) {
            body_pipeline.push_group(o2_pipeline());
        }
        if plan.level.enables(crate::OptimizationLevel::O3) {
            body_pipeline.push_group(o3_pipeline());
        }
        pipeline.push_for_each(
            UnitSelector::Affected(BODY_UNIT_KIND),
            UnitOrder::Stable,
            body_pipeline,
        );
    }
    if plan.quality_context_token_budget.is_some() {
        pipeline.push_pass(ApplyQualityContextBudgetPolicyPass);
    }
    pipeline
}

fn o1_pipeline() -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.hir_optimization.o1")
        .pass(LowerOwnedSnapshotCowPass)
        .pass(FuseSerializationBoundaryPass)
        .pass(SuffixDemandHistoryPass)
        .pass(ProjectAgentObservationPass)
        .pass(PruneSupersededAgentEventsPass)
        .pass(ShortCircuitAgentThresholdPass)
        .pass(PruneUnusedQualityContextPass)
}

fn o2_pipeline() -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.hir_optimization.o2")
        .pass(PrepareVersionedRequestPass)
        .pass(SpecializeAgentToolSchemasPass)
        .pass(SpecializeDeterministicDispatchPass)
        .pass(VersionedPreparationCsePass)
        .pass(FuseProjectionScansPass)
        .pass(BatchStorageSemijoinPass)
        .pass(SynthesizeScopedIndexPass)
}

fn o3_pipeline() -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.hir_optimization.o3")
        .pass(ScheduleBoundedPipelinePass)
        .pass(LowerResourceRegionPass)
}

#[cfg(test)]
mod tests {
    use etas_utils::{ArtifactScope, PassControl, PassManager};

    use super::{optimization_pipeline, project_pipeline};
    use crate::{OptimizationLevel, ProjectContext, ProjectInput, SourceInput};

    const O1_PASSES: &[&str] = &[
        "AgentEffectLegalityPass",
        "LowerOwnedSnapshotCowPass",
        "FuseSerializationBoundaryPass",
        "SuffixDemandHistoryPass",
        "ProjectAgentObservationPass",
        "PruneSupersededAgentEventsPass",
        "ShortCircuitAgentThresholdPass",
        "PruneUnusedQualityContextPass",
    ];
    const O2_PASSES: &[&str] = &[
        "PrepareVersionedRequestPass",
        "SpecializeAgentToolSchemasPass",
        "SpecializeDeterministicDispatchPass",
        "VersionedPreparationCsePass",
        "FuseProjectionScansPass",
        "BatchStorageSemijoinPass",
        "SynthesizeScopedIndexPass",
    ];
    const O3_PASSES: &[&str] = &["ScheduleBoundedPipelinePass", "LowerResourceRegionPass"];

    fn plan(
        level: OptimizationLevel,
        budget: Option<usize>,
    ) -> crate::optimization::OptimizationPlan {
        crate::optimization::OptimizationPlan {
            level,
            quality_context_token_budget: budget,
        }
    }

    fn scheduled_passes(level: OptimizationLevel, budget: Option<usize>) -> Vec<String> {
        let pipeline = optimization_pipeline(&plan(level, budget));
        PassManager::<ProjectContext>::new()
            .schedule_text(&pipeline)
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pass ").map(str::to_owned))
            .collect()
    }

    fn expected_passes(level: OptimizationLevel, budget: Option<usize>) -> Vec<String> {
        let mut passes = Vec::new();
        if level.enables(OptimizationLevel::O1) || budget.is_some() {
            passes.push("AgentEffectLegalityPass");
        }
        if level.enables(OptimizationLevel::O1) {
            passes.extend(O1_PASSES[1..].iter().copied());
        }
        if level.enables(OptimizationLevel::O2) {
            passes.extend(O2_PASSES.iter().copied());
        }
        if level.enables(OptimizationLevel::O3) {
            passes.extend(O3_PASSES.iter().copied());
        }
        if budget.is_some() {
            passes.push("ApplyQualityContextBudgetPolicyPass");
        }
        passes.into_iter().map(str::to_owned).collect()
    }

    #[test]
    fn optimization_schedules_are_exact_monotonic_and_deterministic() {
        for level in [
            OptimizationLevel::O0,
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ] {
            let first = scheduled_passes(level, None);
            let second = scheduled_passes(level, None);
            assert_eq!(first, second, "{level}");
            assert_eq!(first, expected_passes(level, None), "{level}");
        }
        assert_eq!(
            scheduled_passes(OptimizationLevel::O0, Some(8)),
            expected_passes(OptimizationLevel::O0, Some(8))
        );
    }

    #[test]
    fn optimization_records_match_the_selected_level() {
        for level in [
            OptimizationLevel::O0,
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ] {
            let mut input = ProjectInput::single_source(SourceInput::anonymous(
                "flow main() -> i64 { return 1; }",
            ));
            input.options.optimization_level = level;
            let mut context = ProjectContext::new(input);
            let mut pipeline = project_pipeline(&plan(level, None));
            let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
            assert!(matches!(run.control, PassControl::Continue), "{level}");
            let selected = expected_passes(level, None);
            let records = run
                .records
                .iter()
                .filter(|record| selected.iter().any(|name| name == record.pass))
                .map(|record| record.pass)
                .collect::<Vec<_>>();
            assert_eq!(records, selected, "{level}");
            assert_eq!(context.optimization_report.effective_level, level);
        }
    }

    #[test]
    fn pipeline_records_body_scoped_type_and_effect_artifacts() {
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow main() -> i64 {
  return 1;
}
"#,
        )));
        let plan = crate::optimization::OptimizationPlan::from_options(&context.input.options);
        let mut pipeline = project_pipeline(&plan);
        let mut manager = PassManager::new();
        let run = manager.run_pipeline(&mut pipeline, &mut context);

        assert!(
            matches!(run.control, PassControl::Continue),
            "{:?}",
            run.control
        );

        let parse_source_records = run
            .records
            .iter()
            .filter(|record| record.pass == "ParseSourceFilePass")
            .collect::<Vec<_>>();
        assert!(!parse_source_records.is_empty());
        assert!(parse_source_records.iter().all(|record| {
            record.produced.iter_refs().any(|artifact| {
                artifact.key.name == "diagnostics"
                    && matches!(artifact.scope, ArtifactScope::Unit(_))
            })
        }));

        let type_body_records = run
            .records
            .iter()
            .filter(|record| record.pass == "TypeCheckBodyPass")
            .collect::<Vec<_>>();
        assert!(!type_body_records.is_empty());
        assert!(type_body_records.iter().all(|record| {
            record.produced.iter_refs().any(|artifact| {
                artifact.key.name == "type_facts"
                    && matches!(artifact.scope, ArtifactScope::Unit(_))
            })
        }));
        assert!(type_body_records.iter().all(|record| {
            record.produced.iter_refs().any(|artifact| {
                artifact.key.name == "diagnostics"
                    && matches!(artifact.scope, ArtifactScope::Unit(_))
            })
        }));

        let effect_pipeline = run
            .records
            .iter()
            .find(|record| record.pass == "RunEffectPipelinePass")
            .expect("RunEffectPipelinePass record");
        let prepared_registry = run
            .records
            .iter()
            .find(|record| record.pass == "PrepareEffectRegistryPass")
            .expect("PrepareEffectRegistryPass record");
        assert!(prepared_registry.produced.iter_refs().any(|artifact| {
            artifact.key.name == "prepared_effect_registry"
                && artifact.scope == ArtifactScope::Global
        }));
        assert!(effect_pipeline.produced.iter_refs().any(|artifact| {
            artifact.key.name == "effect_facts" && artifact.scope == ArtifactScope::Global
        }));
        assert!(effect_pipeline.produced.iter_refs().any(|artifact| {
            artifact.key.name == "diagnostics" && artifact.scope == ArtifactScope::Global
        }));

        let resolve_entry = run
            .records
            .iter()
            .find(|record| record.pass == "ResolveEntryItemPass")
            .expect("ResolveEntryItemPass record");
        assert!(resolve_entry.produced.iter_refs().any(|artifact| {
            artifact.key.name == "diagnostics" && artifact.scope == ArtifactScope::Global
        }));

        let finalize_type = run
            .records
            .iter()
            .find(|record| record.pass == "FinalizeTypeFactsPass")
            .expect("FinalizeTypeFactsPass record");
        assert!(finalize_type.produced.iter_refs().any(|artifact| {
            artifact.key.name == "type_facts" && artifact.scope == ArtifactScope::Global
        }));

        let pass_position = |name: &str| {
            run.records
                .iter()
                .position(|record| record.pass == name)
                .unwrap_or_else(|| panic!("{name} record"))
        };
        assert!(pass_position("ResolvePathsPass") < pass_position("ResolveEntryItemPass"));
        assert!(
            pass_position("ResolveEntryItemPass") < pass_position("ComputeEntryReachabilityPass")
        );
        assert!(
            pass_position("ComputeEntryReachabilityPass")
                < pass_position("BuildSignatureFactsPass")
        );
        assert!(
            pass_position("PrepareBodyTypeEnvironmentPass")
                < pass_position("PrepareEffectRegistryPass")
        );
        assert!(pass_position("PrepareEffectRegistryPass") < pass_position("TypeCheckBodyPass"));
        assert!(
            pass_position("RunEffectPipelinePass") < pass_position("ValidateEntryContractPass")
        );
        assert!(
            pass_position("RunEffectPipelinePass") < pass_position("AnalyzeConcurrencyPass")
        );
        assert!(
            pass_position("AnalyzeConcurrencyPass") < pass_position("VerifyInterpreterSupportPass")
        );
    }
}
