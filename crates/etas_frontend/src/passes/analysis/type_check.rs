use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{
    HIR_OUTPUT, RESOLVED_IMPORTS, RESOLVED_PATHS, SIGNATURE_FACTS, VALIDATED_EXTERNAL_ENVIRONMENT,
    global_with_diagnostics,
};

use super::type_check_bridge::build_signature_pipeline_input;

pub struct BuildSignatureFactsPass;

impl Pass<ProjectContext> for BuildSignatureFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildSignatureFactsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                RESOLVED_IMPORTS,
                RESOLVED_PATHS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(global_with_diagnostics([SIGNATURE_FACTS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let hir = context.hir.as_ref().expect("HIR output should exist");
        let (input, input_diagnostics) = build_signature_pipeline_input(context, &hir.hir);
        let mut output = etas_types::run_signature_pipeline(input);
        output.diagnostics.extend(input_diagnostics);
        context
            .diagnostics
            .extend(output.diagnostics.iter().cloned());
        context.signature_types = Some(output);
        context.type_body_outputs.clear();
        context.types = None;
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(SIGNATURE_FACTS))
    }
}
