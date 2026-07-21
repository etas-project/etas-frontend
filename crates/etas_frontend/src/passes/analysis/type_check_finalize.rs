use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{SIGNATURE_FACTS, TYPE_OUTPUT};

pub struct FinalizeTypeFactsPass;

impl Pass<ProjectContext> for FinalizeTypeFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("FinalizeTypeFactsPass", PassKind::Transform)
            .requires(ArtifactSet::one(SIGNATURE_FACTS))
            .produces(ArtifactSet::one(TYPE_OUTPUT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let signatures = context
            .signature_types
            .as_ref()
            .cloned()
            .expect("signature facts should exist before finalizing type facts");
        let mut units = context
            .type_body_outputs
            .keys()
            .copied()
            .collect::<Vec<_>>();
        units.sort_by_key(|unit| unit.0);
        let body_outputs = units
            .into_iter()
            .filter_map(|unit| context.type_body_outputs.get(&unit).cloned())
            .collect::<Vec<_>>();
        context.types = Some(etas_types::finalize_type_outputs(signatures, body_outputs));
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(TYPE_OUTPUT))
    }
}
