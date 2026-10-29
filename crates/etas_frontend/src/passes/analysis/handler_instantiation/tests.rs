use super::*;
use etas_utils::{PassControl, Pipeline};
use std::sync::Arc;

#[test]
fn checked_publication_requires_completed_handler_instantiations() {
    let mut manager = PassManager::new();
    let mut context = ProjectContext::new(crate::ProjectInput::single_source(
        crate::SourceInput::anonymous("flow main() -> unit { return; }"),
    ));
    let descriptor = crate::passes::BuildCheckedProjectPass.descriptor();
    for artifact in descriptor.requires.iter_refs() {
        if artifact.key != HANDLER_INSTANTIATIONS {
            manager.mark_artifact_ref_available(artifact);
        }
    }
    let run = manager.run_pipeline(
        &mut Pipeline::new("test.handler_publication").pass(crate::passes::BuildCheckedProjectPass),
        &mut context,
    );
    assert!(matches!(run.control, PassControl::Failed(_)));
    assert!(context.checked.is_none());
}

#[test]
fn missing_request_evidence_cannot_publish_a_checked_program() {
    for missing_summary in [true, false] {
        let mut source = crate::SourceInput::anonymous(
            r#"
module main;
effect Echo { action value<T>(payload: T) -> T; }
flow hidden() -> i32 ![Echo.value] { return perform Echo.value(7); }
flow main() -> i32 { return handle { hidden() } with { Echo.value(_) => { resume 42; } }; }
"#,
        );
        source.id = etas_core::SourceId(271);
        let mut context = ProjectContext::new(crate::ProjectInput::single_source(source));
        let mut manager = PassManager::new();
        let run = manager.run_pipeline(&mut crate::pipeline::project_pipeline(), &mut context);
        assert!(matches!(run.control, PassControl::Continue));
        assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
        assert!(context.checked.is_some());
        let hir = &context.hir.as_ref().unwrap().hir;
        let (arm, body) = hir
            .exprs
            .iter()
            .find_map(|(_, expr)| {
                let etas_hir::HirExpr::Handle { body, handler, .. } = expr else {
                    return None;
                };
                let etas_hir::HirExpr::Handler { handlers, .. } = &hir.exprs[*handler] else {
                    return None;
                };
                Some((handlers[0], *body))
            })
            .unwrap();
        let span = hir.handler_arms[arm].span;
        let types = context.types.as_mut().unwrap();
        let candidate = types.facts.handler_actions[&arm].output;
        Arc::make_mut(&mut types.facts)
            .deferred_handler_instantiations
            .insert(
                arm,
                etas_types::DeferredHandlerInstantiation {
                    body,
                    overload: None,
                    span,
                    parameters: vec![etas_types::HandlerTypeBinding {
                        parameter: "T".into(),
                        candidate,
                        rigid: candidate,
                        parametric: false,
                    }],
                },
            );
        let effects = Arc::make_mut(&mut context.effects.as_mut().unwrap().facts);
        if missing_summary {
            effects.handled_body_requests.clear();
        } else {
            effects
                .handled_body_requests
                .insert(body, etas_effects::RequestInstantiationSummary::Unavailable);
        }
        let run = manager.run_pipeline(
            &mut Pipeline::new("test.corrupted_handler_evidence")
                .pass(ValidateHandlerInstantiationsPass)
                .pass(crate::passes::BuildCheckedProjectPass),
            &mut context,
        );
        assert!(matches!(run.control, PassControl::Continue));
        assert!(context.checked.is_none());
        assert_eq!(
            context
                .types
                .as_ref()
                .unwrap()
                .facts
                .deferred_handler_instantiations
                .len(),
            1
        );
        let diagnostic = context
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code
                    == etas_core::DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)
            })
            .unwrap();
        assert_eq!(diagnostic.primary.span, span);
        assert_eq!(diagnostic.primary.span.source, etas_core::SourceId(271));
    }
}
