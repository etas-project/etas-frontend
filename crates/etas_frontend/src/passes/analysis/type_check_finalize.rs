use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{SIGNATURE_FACTS, TYPE_ENVIRONMENT, TYPE_OUTPUT};

pub struct FinalizeTypeFactsPass;

impl Pass<ProjectContext> for FinalizeTypeFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("FinalizeTypeFactsPass", PassKind::Transform)
            .requires(ArtifactSet::from([SIGNATURE_FACTS, TYPE_ENVIRONMENT]))
            .produces(ArtifactSet::one(TYPE_OUTPUT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let Some(base) = &context.type_environment else {
            return PassResult::failed("type finalization requires prepared signature environment");
        };
        let mut units = context
            .type_body_outputs
            .keys()
            .copied()
            .collect::<Vec<_>>();
        units.sort_by_key(|unit| unit.0);
        let bodies = units
            .iter()
            .map(|unit| context.type_body_outputs[unit].as_ref());
        let bound = context
            .candidate
            .as_ref()
            .and_then(|candidate| candidate.bound_body.as_ref());
        let result = match bound {
            Some(bound) => etas_types::finalize_body_deltas_with_application(base, bodies, bound),
            None if context
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.bound_application.is_some()) =>
            {
                return PassResult::failed("bound candidate body was not checked");
            }
            None => etas_types::finalize_body_deltas(base, bodies),
        };
        match result {
            Ok(output) => context.types = Some(output),
            Err(error) => return PassResult::failed(error),
        }
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(TYPE_OUTPUT))
    }
}
