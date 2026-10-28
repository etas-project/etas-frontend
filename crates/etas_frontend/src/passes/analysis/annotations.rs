use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    passes::artifacts::{
        HIR_OUTPUT, SIGNATURE_FACTS, TOP_LEVEL_LET_FACTS, VALIDATED_ITEM_ANNOTATIONS,
        global_with_diagnostics,
    },
};

pub struct ValidateItemAnnotationsPass;

impl Pass<ProjectContext> for ValidateItemAnnotationsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateItemAnnotationsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                SIGNATURE_FACTS,
                TOP_LEVEL_LET_FACTS,
            ]))
            .produces(global_with_diagnostics([VALIDATED_ITEM_ANNOTATIONS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(hir), Some(types), Some(_)) = (
            &context.hir,
            &context.signature_types,
            &context.top_level_lets,
        ) else {
            return PassResult::failed(
                "annotation validation requires classified HIR and signature facts",
            );
        };
        context
            .diagnostics
            .extend(etas_types::validate_item_annotations(
                &hir.hir,
                &types.facts,
                &context.std_registry,
            ));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(VALIDATED_ITEM_ANNOTATIONS),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectInput, SourceInput, passes::BuildCheckedProjectPass};
    use etas_utils::{PassControl, Pipeline};

    #[test]
    fn checked_project_emission_requires_the_annotation_validation_artifact() {
        let pass = BuildCheckedProjectPass;
        let descriptor = pass.descriptor();
        let mut manager = PassManager::new();
        manager.mark_artifacts_available(&ArtifactSet::from_iter(
            descriptor
                .requires
                .iter_refs()
                .filter(|artifact| artifact.key != VALIDATED_ITEM_ANNOTATIONS),
        ));
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput::anonymous(
            "flow main() -> unit { return; }",
        )));
        let mut pipeline = Pipeline::new("frontend.annotation_gate").pass(pass);
        let result = manager.run_pipeline(&mut pipeline, &mut context);
        let PassControl::Failed(failure) = result.control else {
            panic!("checked project was emitted without annotation validation")
        };
        assert_eq!(failure.missing_artifact, Some(VALIDATED_ITEM_ANNOTATIONS));
        assert!(context.checked.is_none());
    }
}
