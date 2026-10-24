use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    passes::artifacts::{HIR_OUTPUT, SIGNATURE_FACTS, TOP_LEVEL_LET_FACTS, TYPE_ENVIRONMENT},
};

pub struct PrepareBodyTypeEnvironmentPass;

impl Pass<ProjectContext> for PrepareBodyTypeEnvironmentPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PrepareBodyTypeEnvironmentPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                SIGNATURE_FACTS,
                TOP_LEVEL_LET_FACTS,
            ]))
            .produces(ArtifactSet::one(TYPE_ENVIRONMENT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(hir), Some(signatures)) = (&context.hir, &context.signature_types) else {
            return PassResult::failed("body type environment requires HIR and signature facts");
        };
        context.type_environment =
            Some(etas_types::SignatureEnvironment::new(&hir.hir, signatures));
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(TYPE_ENVIRONMENT))
    }
}
