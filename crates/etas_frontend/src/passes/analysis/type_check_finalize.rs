use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;
use crate::passes::artifacts::{HIR_OUTPUT, SIGNATURE_FACTS, TYPE_ENVIRONMENT, TYPE_OUTPUT};

pub struct FinalizeTypeFactsPass;

impl Pass<ProjectContext> for FinalizeTypeFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("FinalizeTypeFactsPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                SIGNATURE_FACTS,
                TYPE_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::from([HIR_OUTPUT, TYPE_OUTPUT]))
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
            Ok(mut output) => {
                if let Err(error) = super::collection_literals::reify(
                    context.hir.as_mut().expect("type checking has HIR"),
                    &mut output,
                ) {
                    return PassResult::failed(error);
                }
                context
                    .diagnostics
                    .extend(super::standard_library::execution_role_diagnostics(
                        &context.hir.as_ref().unwrap().hir,
                        &context.std_registry,
                        &output,
                    ));
                let hir = &context.hir.as_ref().unwrap().hir;
                let layouts =
                    match etas_hir_analysis::closures::analyze_runtime_captures(&hir.view()) {
                        Ok(layouts) => layouts,
                        Err(reason) => return PassResult::failed(reason),
                    };
                match super::lexical_capture::escape_diagnostics(hir, &output.facts, &layouts) {
                    Ok(diagnostics) => context.diagnostics.extend(diagnostics),
                    Err(reason) => return PassResult::failed(reason),
                }
                match super::session_lease::check(hir, &output, &layouts) {
                    Ok(diagnostics) => context.diagnostics.extend(diagnostics),
                    Err(reason) => return PassResult::failed(reason),
                }
                context.types = Some(output);
            }
            Err(error) => return PassResult::failed(error),
        }
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from([HIR_OUTPUT, TYPE_OUTPUT]),
        )
    }
}
