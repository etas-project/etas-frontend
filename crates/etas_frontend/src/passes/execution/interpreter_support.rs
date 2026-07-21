use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_effects::{FrontendRejectionReason, InterpreterSupport};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::ProjectContext;

use crate::passes::{
    artifacts::{EFFECT_OUTPUT, HIR_OUTPUT, INTERPRETER_SUPPORT, global_with_diagnostics},
    common::frontend_span,
};

pub struct VerifyInterpreterSupportPass;

impl Pass<ProjectContext> for VerifyInterpreterSupportPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("VerifyInterpreterSupportPass", PassKind::Verify)
            .requires(ArtifactSet::from([HIR_OUTPUT, EFFECT_OUTPUT]))
            .produces(global_with_diagnostics([INTERPRETER_SUPPORT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context.input.options.entry_policy != crate::EntryPolicy::Runnable {
            return PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::one(INTERPRETER_SUPPORT),
            );
        }
        let Some(effects) = context.effects.as_ref() else {
            return PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::one(INTERPRETER_SUPPORT),
            );
        };
        let diagnostics = effects
            .facts
            .interpreter_support
            .items
            .iter()
            .filter(|(item_id, _)| item_is_in_readiness_scope(context, **item_id))
            .filter_map(|(item_id, support)| {
                let span = context
                    .hir
                    .as_ref()
                    .and_then(|hir| hir.hir.items.get(*item_id).map(|item| item.span()))
                    .unwrap_or_else(|| frontend_span(context));
                match support {
                    InterpreterSupport::LocalOnly => None,
                    InterpreterSupport::RequiresHost(requirements) => {
                        Some(non_blocking_readiness_diagnostic(
                            span,
                            &format!(
                                "checked HIR execution requires host support: {:?}",
                                requirements.kinds
                            ),
                        ))
                    }
                    InterpreterSupport::RequiresInterpreterOrchestration(requirements) => {
                        Some(non_blocking_readiness_diagnostic(
                            span,
                            &format!(
                                "checked HIR execution requires interpreter orchestration: host={:?}, features={:?}",
                                requirements.host.kinds,
                                requirements.features.kinds
                            ),
                        ))
                    }
                    InterpreterSupport::Rejected(reason) => Some(Diagnostic::effect_check(
                        EffectDiagnosticCode::EscapedEffect,
                        span,
                        frontend_rejection_message(reason),
                    )),
                }
            })
            .collect::<Vec<_>>();

        if let Some(effects) = context.effects.as_mut() {
            effects.diagnostics.extend(diagnostics.iter().cloned());
        }
        context.diagnostics.extend(diagnostics);

        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(INTERPRETER_SUPPORT),
        )
    }
}

fn frontend_rejection_message(reason: &FrontendRejectionReason) -> String {
    format!("frontend rejected this effect behavior: {reason:?}")
}

fn non_blocking_readiness_diagnostic(span: etas_core::Span, message: &str) -> Diagnostic {
    let mut diagnostic =
        Diagnostic::effect_check(EffectDiagnosticCode::RuntimeRequiredInPhase1, span, message);
    diagnostic.severity = etas_core::Severity::Warning;
    diagnostic
}

fn item_is_in_readiness_scope(context: &ProjectContext, item: etas_hir::HirItemId) -> bool {
    if context.check_scope != crate::CheckScope::EntryReachable {
        return true;
    }
    context
        .reachability
        .as_ref()
        .is_some_and(|reachability| reachability.reachable_items.contains(&item))
}
