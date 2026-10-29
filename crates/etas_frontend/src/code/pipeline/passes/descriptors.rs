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
    pub applied_contract: Option<Arc<crate::code::definition::FrozenDefinitionContract>>,
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
        let applied_contract = match (
            &catalog,
            &candidate.bound_application,
            &candidate.bound_body,
        ) {
            (Some(catalog), Some(application), Some(body)) => {
                if body.owner() != application.owner || body.arguments() != application.arguments {
                    return PassResult::failed(
                        "candidate bound body does not match its application",
                    );
                }
                let Some(signature) = crate::project::instantiation::signature(
                    &catalog.checked_program().types,
                    application.owner,
                ) else {
                    return PassResult::failed(
                        "candidate bound application has no generic signature",
                    );
                };
                if signature.generic_params.len() != application.arguments.len() {
                    return PassResult::failed(
                        "candidate bound application has inconsistent arguments",
                    );
                }
                let mut types = Vec::new();
                let mut rows = Vec::new();
                for (parameter, argument) in
                    signature.generic_params.iter().zip(&application.arguments)
                {
                    match (parameter.kind, argument) {
                        (
                            etas_types::CallableGenericParamKind::Type,
                            etas_types::CallableGenericArg::Type(ty),
                        ) => types.push((parameter.name.clone(), *ty)),
                        (
                            etas_types::CallableGenericParamKind::Effect,
                            etas_types::CallableGenericArg::EffectRow(row),
                        ) => rows.push((parameter.name.clone(), row.clone())),
                        _ => {
                            return PassResult::failed(
                                "candidate bound application has wrong-kind argument",
                            );
                        }
                    }
                }
                let application = match catalog.checked_program().checked_callable_application(
                    application.owner,
                    &types,
                    &rows,
                ) {
                    Ok(application) => application,
                    Err(error) => return PassResult::failed(error.to_string()),
                };
                match crate::code::definition::FrozenDefinitionContract::capture_application(
                    catalog,
                    &application,
                    limits,
                ) {
                    Ok(contract) => Some(contract),
                    Err(error) if error.resource_limit().is_some() => {
                        return PassResult::interrupted(etas_utils::PipelineInterruption {
                            kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                            reason: error.to_string(),
                        });
                    }
                    Err(error) => return PassResult::failed(error.to_string()),
                }
            }
            (None, Some(_), _) => None,
            (Some(_), Some(_), None) | (_, None, Some(_)) => {
                return PassResult::failed("candidate application and bound body facts disagree");
            }
            (_, None, None) => None,
        };
        candidate.descriptors = Some(CandidateDescriptors {
            catalog,
            applied_contract,
            limits,
        });
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
