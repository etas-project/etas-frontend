use etas_core::{Diagnostic, Severity, TypeDiagnosticCode};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    passes::artifacts::{
        DIAGNOSTICS, EFFECT_OUTPUT, GENERIC_EFFECT_BINDINGS, HANDLER_INSTANTIATIONS, TYPE_OUTPUT,
    },
};

mod requests;
#[cfg(test)]
mod tests;

pub struct ValidateHandlerInstantiationsPass;

impl Pass<ProjectContext> for ValidateHandlerInstantiationsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateHandlerInstantiationsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                GENERIC_EFFECT_BINDINGS,
                DIAGNOSTICS,
            ]))
            .produces(ArtifactSet::from([
                HANDLER_INSTANTIATIONS,
                TYPE_OUTPUT,
                DIAGNOSTICS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
        }
        let (Some(types), Some(effects), Some(artifacts)) = (
            &mut context.types,
            &context.effects,
            &context.effect_pipeline_artifacts,
        ) else {
            return PassResult::failed(
                "handler instantiation requires checked HIR, type and effect analysis artifacts",
            );
        };
        let mut obligations = types
            .facts
            .deferred_handler_instantiations
            .iter()
            .collect::<Vec<_>>();
        obligations.sort_by_key(|(arm, _)| **arm);
        let mut proofs = Vec::new();
        for (arm, obligation) in obligations {
            if let Some(control) = manager.execution_control()
                && let Err(error) = control.checkpoint()
            {
                return PassResult::interrupted(error);
            }
            let proof = if obligation
                .parameters
                .iter()
                .all(|parameter| parameter.parametric)
            {
                Ok(Vec::new())
            } else {
                requests::collect(
                    requests::RequestInput {
                        types,
                        effects: &effects.facts,
                        registry: &artifacts.registry,
                        control: manager.execution_control(),
                    },
                    *arm,
                    obligation.body,
                )
            };
            match proof {
                Ok(requests) => proofs.push(etas_types::HandlerInstantiationProof {
                    arm: *arm,
                    requests,
                }),
                Err(requests::RequestError::Interrupted(error)) => {
                    return PassResult::interrupted(error);
                }
                Err(error) => context.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    obligation.span,
                    error.to_string(),
                )),
            }
        }
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return PassResult::changed(PreservedArtifacts::All, self.descriptor().produces);
        }
        if let Err(failures) = etas_types::complete_handler_instantiations(types, &proofs) {
            context
                .diagnostics
                .extend(failures.into_iter().map(|failure| {
                    Diagnostic::type_check(failure.code, failure.span, failure.message)
                }));
        }
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}
