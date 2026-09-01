use etas_utils::{Pipeline, UnitOrder, UnitSelector};

use crate::passes::artifacts::TYPE_OUTPUT;
use crate::passes::{
    AnalyzeLoopProgressPass, ApplyResolvedImportsToHirPass, BuildCheckedProjectPass,
    BuildImportGraphPass, BuildModuleCatalogPass, BuildModuleIndexPass, BuildSignatureFactsPass,
    BuildSourceSetPass, BuildUnitTreePass, ComputeAffectedModulesPass,
    ComputeEntryReachabilityPass, ComputeModuleTopoOrderPass, DetectImportCyclesPass,
    FinalizeProjectHirPass, FinalizeTypeFactsPass, LowerModuleItemsPass,
    NormalizeModuleImportsPass, ParseSourceFilePass, PredeclareProjectSymbolsPass,
    ResolveEntryItemPass, ResolveImportTargetsPass, ResolvePathsPass, ReuseTypeBodyFactsPass,
    RunEffectPipelinePass, TypeCheckBodyPass, ValidateEntryContractPass,
    ValidateExternalEnvironmentPass, ValidateTopLevelLetPass, VerifyInterpreterSupportPass,
};
use crate::{
    BODY_UNIT_KIND, MODULE_PART_UNIT_KIND, ProjectContext, SOURCE_FILE_UNIT_KIND,
    project::ENTRY_REACHABLE_UNIT_FILTER,
};

pub fn project_pipeline() -> Pipeline<ProjectContext> {
    Pipeline::new("frontend.check_project")
        .pass(BuildSourceSetPass)
        .for_each(
            UnitSelector::Kind(SOURCE_FILE_UNIT_KIND),
            UnitOrder::SourceOrder,
            Pipeline::new("frontend.source").pass(ParseSourceFilePass),
        )
        .pass(BuildModuleIndexPass)
        .pass(BuildModuleCatalogPass)
        .pass(ResolveImportTargetsPass)
        .pass(ValidateExternalEnvironmentPass)
        .pass(BuildUnitTreePass)
        .pass(BuildImportGraphPass)
        .pass(DetectImportCyclesPass)
        .pass(ComputeModuleTopoOrderPass)
        .pass(ComputeAffectedModulesPass)
        .pass(PredeclareProjectSymbolsPass)
        .for_each(
            UnitSelector::Kind(MODULE_PART_UNIT_KIND),
            UnitOrder::DependencyOrder,
            Pipeline::new("frontend.module_part")
                .pass(NormalizeModuleImportsPass)
                .pass(LowerModuleItemsPass),
        )
        .pass(FinalizeProjectHirPass)
        .pass(ApplyResolvedImportsToHirPass)
        .pass(ResolvePathsPass)
        .pass(ResolveEntryItemPass)
        .pass(ComputeEntryReachabilityPass)
        .pass(BuildSignatureFactsPass)
        .pass(ValidateTopLevelLetPass)
        .pass(ReuseTypeBodyFactsPass)
        .for_each(
            UnitSelector::AffectedArtifact {
                kind: BODY_UNIT_KIND,
                artifact: TYPE_OUTPUT,
                filter: Some(ENTRY_REACHABLE_UNIT_FILTER),
            },
            UnitOrder::Stable,
            Pipeline::new("frontend.type_body").pass(TypeCheckBodyPass),
        )
        .pass(FinalizeTypeFactsPass)
        .pass(RunEffectPipelinePass)
        .pass(AnalyzeLoopProgressPass)
        .pass(VerifyInterpreterSupportPass)
        .pass(ValidateEntryContractPass)
        .pass(BuildCheckedProjectPass)
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
            pass_position("RunEffectPipelinePass") < pass_position("ValidateEntryContractPass")
        );
    }
}
