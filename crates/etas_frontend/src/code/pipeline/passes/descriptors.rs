use std::sync::Arc;

use etas_code::descriptor::{DescriptorError, DescriptorLimits};
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    code::{definition::DefinitionCatalog, descriptor::ProjectionError},
    passes::artifacts::CHECKED_PROJECT,
};

use super::{
    CANDIDATE_INVOCATION_TYPES, sources::FROZEN_DEFINITION_ENVIRONMENT, target::CANDIDATE_TARGET,
};

pub(crate) const CANDIDATE_DESCRIPTORS: ArtifactKey =
    ArtifactKey::new("code", "candidate_descriptors");

pub(crate) struct CandidateDescriptors {
    pub catalog: Option<Arc<DefinitionCatalog>>,
    pub limits: DescriptorLimits,
}

pub(crate) struct BuildDefinitionDescriptorsPass;

impl Pass<ProjectContext> for BuildDefinitionDescriptorsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildDefinitionDescriptorsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                CHECKED_PROJECT,
                FROZEN_DEFINITION_ENVIRONMENT,
                CANDIDATE_TARGET,
                CANDIDATE_INVOCATION_TYPES,
            ]))
            .produces(ArtifactSet::one(CANDIDATE_DESCRIPTORS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = &mut context.candidate else {
            return PassResult::failed(
                "candidate descriptors have no frozen definition environment",
            );
        };
        if candidate.descriptors.is_some() {
            return PassResult::failed("candidate descriptors were already materialized");
        }
        let limits = candidate.descriptor_limits;
        let catalog = match &context.checked {
            None => None,
            Some(checked) => {
                let build = candidate
                    .overlay
                    .frozen()
                    .origins_for(checked)
                    .and_then(|origins| {
                        DefinitionCatalog::build(checked.clone(), &origins, limits)
                    });
                match build {
                    Ok(catalog) => Some(Arc::new(catalog)),
                    Err(
                        error @ (ProjectionError::Limit { .. }
                        | ProjectionError::Descriptor(DescriptorError::Limit { .. })),
                    ) => {
                        return PassResult::interrupted(etas_utils::PipelineInterruption {
                            kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                            reason: error.to_string(),
                        });
                    }
                    Err(error) => return PassResult::failed(error.to_string()),
                }
            }
        };
        candidate.descriptors = Some(CandidateDescriptors { catalog, limits });
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CANDIDATE_DESCRIPTORS),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_projection_requires_real_checked_candidate_artifacts() {
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(""),
        ));
        let mut pipeline = etas_utils::Pipeline::new("missing_descriptor_inputs")
            .pass(BuildDefinitionDescriptorsPass);
        let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
        let etas_utils::PassControl::Failed(failure) = run.control else {
            panic!("missing descriptor artifacts must fail")
        };
        assert!(
            BuildDefinitionDescriptorsPass
                .descriptor()
                .requires
                .contains(failure.missing_artifact.unwrap())
        );
        assert!(context.candidate.is_none());
    }
}
