use etas_utils::{Pipeline, UnitOrder, UnitSelector};

use crate::passes::artifacts::TYPE_OUTPUT;
use crate::passes::{
    AnalyzeLoopProgressPass, ApplyResolvedImportsToHirPass, BuildCheckedProjectPass,
    BuildImportGraphPass, BuildModuleCatalogPass, BuildModuleIndexPass, BuildSignatureFactsPass,
    BuildSourceSetPass, BuildUnitTreePass, CompleteTypedAnalysisReachabilityPass,
    CompleteTypedRuntimeReachabilityPass, ComputeAffectedModulesPass,
    ComputeDefinitionAnalysisReachabilityPass, ComputeEntryReachabilityPass,
    ComputeModuleTopoOrderPass, DetectImportCyclesPass, FinalizeProjectHirPass,
    FinalizeTypeFactsPass, InferCallableOutputsPass, LowerModuleItemsPass,
    MaterializeGenericEffectRowsPass, NormalizeModuleImportsPass, ParseSourceFilePass,
    PredeclareProjectSymbolsPass, PrepareBodyTypeEnvironmentPass, PrepareEffectRegistryPass,
    PrepareInvocationTypesPass, ResolveEntryItemPass, ResolveImportTargetsPass, ResolvePathsPass,
    RetainDefinitionSourcesPass, ReuseTypeBodyFactsPass, RunEffectPipelinePass, TypeCheckBodyPass,
    ValidateEntryContractPass, ValidateExternalEnvironmentPass, ValidateHandlerInstantiationsPass,
    ValidateItemAnnotationsPass, ValidateTopLevelLetPass, VerifyInterpreterSupportPass,
};
use crate::{
    BODY_UNIT_KIND, MODULE_PART_UNIT_KIND, ProjectContext, SOURCE_FILE_UNIT_KIND,
    project::ANALYZED_BODY_UNIT_FILTER,
};

pub fn project_pipeline() -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.check_project")
        .group(
            Pipeline::new("frontend.project_sources")
                .pass(BuildSourceSetPass)
                .for_each(
                    UnitSelector::Kind(SOURCE_FILE_UNIT_KIND),
                    UnitOrder::SourceOrder,
                    Pipeline::new("frontend.source").pass(ParseSourceFilePass),
                ),
        )
        .group(semantic_pipeline(false))
        .pass(BuildCheckedProjectPass)
}

pub(crate) fn candidate_body_pipeline() -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.check_candidate_body")
        .pass(crate::code::pipeline::passes::FreezeDefinitionEnvironmentPass)
        .pass(crate::code::pipeline::passes::ParseCandidateDependencySourcesPass)
        .group(semantic_pipeline(true))
        .pass(BuildCheckedProjectPass)
        .pass(crate::code::pipeline::passes::MaterializeCandidateInvocationTypesPass)
        .pass(crate::code::pipeline::passes::BuildDefinitionDescriptorsPass)
        .pass(crate::code::pipeline::passes::ValidateCandidateContractPass)
        .pass(crate::code::pipeline::passes::EmitCheckedCandidatePass)
}

fn semantic_pipeline(candidate: bool) -> Pipeline<ProjectContext> {
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
        .pass(PrepareEffectRegistryPass);
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
        .pass(RunEffectPipelinePass)
        .pass(MaterializeGenericEffectRowsPass)
        .pass(ValidateHandlerInstantiationsPass)
        .pass(AnalyzeLoopProgressPass)
        .pass(VerifyInterpreterSupportPass);
    if !candidate {
        pipeline.push_pass(ValidateEntryContractPass);
    }
    pipeline
        .pass(PrepareInvocationTypesPass)
        .pass(crate::code::pipeline::passes::ResolveReflectionTargetsPass)
}

#[cfg(test)]
mod tests {
    use etas_utils::{ArtifactScope, PassControl, PassManager};

    use super::project_pipeline;
    use crate::{ProjectContext, ProjectInput, SourceInput};

    #[test]
    fn pipeline_records_body_scoped_type_and_effect_artifacts() {
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow main() -> i64 {
  return 1;
}
"#,
        )));
        let mut pipeline = project_pipeline();
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
    }
}
