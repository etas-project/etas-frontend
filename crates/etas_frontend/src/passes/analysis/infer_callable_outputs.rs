use crate::{
    ProjectContext,
    passes::artifacts::{
        CALLABLE_OUTPUT_FACTS, HIR_OUTPUT, SIGNATURE_FACTS, TOP_LEVEL_LET_FACTS,
        global_with_diagnostics,
    },
};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

pub struct InferCallableOutputsPass;

impl Pass<ProjectContext> for InferCallableOutputsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("InferCallableOutputsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                SIGNATURE_FACTS,
                TOP_LEVEL_LET_FACTS,
            ]))
            .produces(global_with_diagnostics([CALLABLE_OUTPUT_FACTS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(hir), Some(mut signatures)) = (&context.hir, context.signature_types.take())
        else {
            return PassResult::failed(
                "callable output inference requires HIR and declaration signatures",
            );
        };
        signatures.diagnostics.clear();
        let output = match manager.execution_control() {
            Some(control) => match etas_types::infer_callable_outputs_controlled(
                &hir.hir,
                signatures,
                context.std_registry.clone(),
                control,
            ) {
                Ok(output) => output,
                Err(interruption) => return PassResult::interrupted(interruption),
            },
            None => etas_types::infer_callable_outputs(
                &hir.hir,
                signatures,
                context.std_registry.clone(),
            ),
        };
        context
            .diagnostics
            .extend(output.diagnostics.iter().cloned());
        context.signature_types = Some(output);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(CALLABLE_OUTPUT_FACTS),
        )
    }
}
