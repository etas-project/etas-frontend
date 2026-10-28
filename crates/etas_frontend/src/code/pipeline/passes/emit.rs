use etas_core::Severity;
use etas_effects::InterpreterSupport;
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use super::{
    CANDIDATE_CONTRACT_ANALYSIS, sources::FROZEN_DEFINITION_ENVIRONMENT, target::CANDIDATE_TARGET,
};
use crate::{
    ProjectContext,
    code::{
        contract::CandidateContractAnalysis,
        service::{CandidateEmission, CheckedCandidate},
    },
    passes::artifacts::{CHECKED_PROJECT, INTERPRETER_SUPPORT},
};

pub(crate) const CANDIDATE_EMISSION: ArtifactKey = ArtifactKey::new("code", "candidate_emission");
pub(crate) struct EmitCheckedCandidatePass;

impl Pass<ProjectContext> for EmitCheckedCandidatePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("EmitCheckedCandidatePass", PassKind::Emit)
            .requires(ArtifactSet::from([
                CHECKED_PROJECT,
                INTERPRETER_SUPPORT,
                FROZEN_DEFINITION_ENVIRONMENT,
                CANDIDATE_TARGET,
                CANDIDATE_CONTRACT_ANALYSIS,
                super::CANDIDATE_INVOCATION_TYPES,
                super::CANDIDATE_DESCRIPTORS,
            ]))
            .produces(ArtifactSet::one(CANDIDATE_EMISSION))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(candidate) = &mut context.candidate else {
            return PassResult::failed("candidate emission has no frozen environment");
        };
        if candidate.emission.is_some() {
            return PassResult::failed("candidate emission was already decided");
        }
        let Some(analysis) = &candidate.contract_analysis else {
            return PassResult::failed("candidate emission has no contract artifact");
        };
        let Some(descriptors) = &candidate.descriptors else {
            return PassResult::failed("candidate emission has no descriptor artifact");
        };
        if descriptors.limits != candidate.descriptor_limits {
            return PassResult::failed(
                "candidate emission descriptor limits differ from the request",
            );
        }
        let proven = match analysis
            .validate_for(candidate.snapshot, candidate.overlay.frozen().contract())
        {
            Ok(proven) => proven,
            Err(error) => return PassResult::failed(error.to_string()),
        };
        let Some(checked) = &context.checked else {
            if !matches!(analysis, CandidateContractAnalysis::SourceRejected { .. })
                || !context
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Error)
            {
                return PassResult::failed("candidate source rejection has no compiler diagnostic");
            }
            candidate.emission = Some(CandidateEmission::Rejected);
            return PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::one(CANDIDATE_EMISSION),
            );
        };
        if matches!(analysis, CandidateContractAnalysis::SourceRejected { .. }) {
            return PassResult::failed(
                "source rejection contradicts the checked candidate program",
            );
        }
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return PassResult::failed("checked candidate still has blocking diagnostics");
        }
        let Some(entry) = candidate
            .target
            .filter(|target| Some(*target) == checked.entry)
        else {
            return PassResult::failed("candidate emission target differs from checked entry");
        };
        let support = &checked.effects.interpreter_support;
        let (Some(item_support), Some(entry_support)) = (support.items.get(&entry), &support.entry)
        else {
            return PassResult::failed(
                "candidate emission requires checked backend support for its entry",
            );
        };
        if matches!(item_support, InterpreterSupport::Rejected(_))
            || matches!(entry_support, InterpreterSupport::Rejected(_))
        {
            return PassResult::failed("candidate backend rejected its checked entry");
        }
        if !proven {
            candidate.emission = Some(CandidateEmission::Rejected);
            return PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::one(CANDIDATE_EMISSION),
            );
        }
        candidate.emission = Some(CandidateEmission::Ready(Box::new(CheckedCandidate {
            descriptor_limits: descriptors.limits,
            descriptors: match &descriptors.catalog {
                Some(catalog) if std::sync::Arc::ptr_eq(catalog.checked_program(), checked) => {
                    catalog.clone()
                }
                _ => {
                    return PassResult::failed(
                        "candidate emission descriptor identity differs from its checked program",
                    );
                }
            },
            program: checked.clone(),
            frozen: candidate.overlay.frozen().clone(),
            revision: candidate.overlay.revision().clone(),
            snapshot: candidate.snapshot,
            entry,
        })));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CANDIDATE_EMISSION),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_is_gated_by_real_pass_manager_artifacts() {
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(""),
        ));
        let mut pipeline =
            etas_utils::Pipeline::new("missing_emit_inputs").pass(EmitCheckedCandidatePass);
        let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
        let etas_utils::PassControl::Failed(failure) = run.control else {
            panic!("missing artifacts must fail")
        };
        assert!(
            EmitCheckedCandidatePass
                .descriptor()
                .requires
                .contains(failure.missing_artifact.unwrap())
        );
        assert!(context.candidate.is_none());
    }
}
