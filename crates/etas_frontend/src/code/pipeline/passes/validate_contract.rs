use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use super::overlay::CANDIDATE_BODY_OVERLAYS_APPLIED;
use super::sources::FROZEN_DEFINITION_ENVIRONMENT;
use super::target::CANDIDATE_TARGET;
use crate::{
    ProjectContext, code::contract::CandidateContractAnalysis, passes::artifacts::CHECKED_PROJECT,
};

pub(crate) const CANDIDATE_CONTRACT_ANALYSIS: ArtifactKey =
    ArtifactKey::new("code", "candidate_contract_analysis");

pub(crate) struct ValidateCandidateContractPass;

impl Pass<ProjectContext> for ValidateCandidateContractPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateCandidateContractPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                CHECKED_PROJECT,
                FROZEN_DEFINITION_ENVIRONMENT,
                CANDIDATE_TARGET,
                CANDIDATE_BODY_OVERLAYS_APPLIED,
                super::CANDIDATE_INVOCATION_TYPES,
                super::CANDIDATE_DESCRIPTORS,
            ]))
            .produces(ArtifactSet::one(CANDIDATE_CONTRACT_ANALYSIS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = &mut context.candidate else {
            return PassResult::failed("candidate contract has no frozen definition environment");
        };
        let snapshot = candidate.snapshot;
        let Some(descriptors) = &candidate.descriptors else {
            return PassResult::failed("candidate contract has no descriptor artifact");
        };
        if descriptors.limits != candidate.descriptor_limits {
            return PassResult::failed(
                "candidate descriptor artifact was built under different limits",
            );
        }
        if context.checked.is_some() != descriptors.catalog.is_some() {
            return PassResult::failed(
                "candidate descriptors disagree with checked source availability",
            );
        }
        if let Some(checked) = &context.checked
            && (candidate.target.is_none() || candidate.target != checked.entry)
        {
            return PassResult::failed(
                "candidate checked entry differs from its resolved body target",
            );
        }
        let analysis = match &context.checked {
            None => CandidateContractAnalysis::SourceRejected { snapshot },
            Some(checked) => match crate::code::contract::analyze(
                &candidate.overlay,
                checked,
                match descriptors.catalog.as_deref() {
                    Some(catalog) if std::sync::Arc::ptr_eq(catalog.checked_program(), checked) => {
                        catalog
                    }
                    _ => {
                        return PassResult::failed(
                            "candidate descriptors belong to a different checked program",
                        );
                    }
                },
                snapshot,
                Default::default(),
                candidate.baseline.as_ref(),
            ) {
                Ok(analysis) => analysis,
                Err(error) => return PassResult::failed(error.to_string()),
            },
        };
        candidate.contract_analysis = Some(analysis);
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_dependencies_are_enforced_before_pass_execution() {
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(""),
        ));
        let mut pipeline = etas_utils::Pipeline::new("missing_contract_inputs")
            .pass(ValidateCandidateContractPass);
        let result = PassManager::new().run_pipeline(&mut pipeline, &mut context);
        let etas_utils::PassControl::Failed(failure) = result.control else {
            panic!("missing dependencies must fail")
        };
        let artifact = failure.missing_artifact.unwrap();
        assert!(
            [
                CHECKED_PROJECT,
                FROZEN_DEFINITION_ENVIRONMENT,
                CANDIDATE_TARGET,
                CANDIDATE_BODY_OVERLAYS_APPLIED,
                super::super::CANDIDATE_INVOCATION_TYPES,
                super::super::CANDIDATE_DESCRIPTORS,
            ]
            .contains(&artifact)
        );
        assert!(context.candidate.is_none());
    }
}
