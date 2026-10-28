use super::sources::CANDIDATE_BODY_OVERLAYS;
use crate::{ProjectContext, passes::artifacts::PREDECLARED_PROJECT_SYMBOLS};
use etas_utils::{
    ArtifactKey, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

pub(crate) const CANDIDATE_BODY_OVERLAYS_APPLIED: ArtifactKey =
    ArtifactKey::new("code", "body_overlays_applied");
pub(crate) struct ApplyCandidateBodyOverlaysPass;

impl Pass<ProjectContext> for ApplyCandidateBodyOverlaysPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ApplyCandidateBodyOverlaysPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                PREDECLARED_PROJECT_SYMBOLS,
                CANDIDATE_BODY_OVERLAYS,
            ]))
            .produces(ArtifactSet::one(CANDIDATE_BODY_OVERLAYS_APPLIED))
    }
    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(candidate), Some(lowering)) = (&context.candidate, &mut context.hir_lowering)
        else {
            return PassResult::failed(
                "candidate body lowering requires its frozen overlay and predeclared symbols",
            );
        };
        let mut revisions = candidate.revisions.values().collect::<Vec<_>>();
        revisions.sort_by_key(|revision| {
            (
                revision.declaration.source.0,
                revision.declaration.range.start.0,
            )
        });
        for revision in revisions {
            if let Err(error) = lowering
                .lowering
                .replace_body(revision.declaration, revision.relocated.clone())
            {
                return PassResult::failed(format!(
                    "cannot install candidate body revision: {error:?}"
                ));
            }
        }
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CANDIDATE_BODY_OVERLAYS_APPLIED),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_overlay_dependencies_are_enforced_by_the_pass_manager() {
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(
            crate::SourceInput::anonymous(""),
        ));
        let mut pipeline = etas_utils::Pipeline::new("missing_candidate_artifacts")
            .pass(ApplyCandidateBodyOverlaysPass);
        let result = PassManager::new().run_pipeline(&mut pipeline, &mut context);
        let etas_utils::PassControl::Failed(failure) = result.control else {
            panic!("missing dependency must fail before executing the pass");
        };
        assert_eq!(failure.missing_artifact, Some(CANDIDATE_BODY_OVERLAYS));
    }
}
