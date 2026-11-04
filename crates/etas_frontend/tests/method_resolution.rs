use etas_frontend::{CheckRequest, CheckScope, FrontendSession, ProjectInput, SourceInput};
use etas_types::method::MethodTarget;

#[test]
fn delayed_method_argument_inference_preserves_the_callback_effect_contract() {
    let source = include_str!("fixtures/method_resolution/contextual_arguments.es");
    let aliased = source
        .replace(
            "flow main() -> i32 ![] {",
            "flow main() -> i32 ![] { let callback = item => { return item.read_value(); };",
        )
        .replace(
            "next = item => { return item.read_value(); }",
            "next = callback",
        );
    for source in [source.to_owned(), aliased] {
        let output = etas_frontend::Frontend
            .check_project(ProjectInput::single_source(SourceInput::anonymous(&source)));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert!(summary.escaping_effects.is_empty());
        assert!(summary.requested_actions.effects.is_empty());
        let effectful = source
            .replace(
                "flow main() -> i32 ![]",
                "flow main() -> i32 ![Console.stdout_write]",
            )
            .replace(
                "return item.read_value();",
                "perform Console.stdout_write(\"not pure\"); return item.read_value();",
            );
        let output = etas_frontend::Frontend.check_project(ProjectInput::single_source(
            SourceInput::anonymous(effectful),
        ));
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code
                    == etas_core::DiagnosticCode::Effect(
                        etas_core::EffectDiagnosticCode::EscapedEffect,
                    )
                    && diagnostic.severity == etas_core::Severity::Error
            }),
            "{:?}",
            output.diagnostics
        );
    }
}

#[test]
fn explicit_method_rows_wait_for_callback_behavior_and_publish_one_instantiated_abi() {
    let source = include_str!("fixtures/method_resolution/callback_rows.es");
    for source in [source.to_owned(), source.replace(".run<", "::Runner.run<")] {
        let output = etas_frontend::Frontend
            .check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert_eq!(summary.escaping_effects.effects.iter().count(), 1);
        assert_eq!(summary.requested_actions.effects.iter().count(), 1);
        assert!(
            matches!(&summary.action_trace, etas_effects::ActionTraceDomain::Seq(events)
            if events.len() == 2 && events.iter().all(|event| matches!(event, etas_effects::ActionTraceDomain::Event(_)))),
            "{:?}",
            summary.action_trace
        );
        for call in checked.types.checked_method_calls.values() {
            assert!(
                call.instantiation
                    .deferred_effect_row_obligations
                    .is_empty()
            );
            let Some(etas_types::Type::Function(callback)) =
                checked.type_store.get(call.signature.params[1])
            else {
                panic!("checked callback ABI");
            };
            let row = callback.effects.as_ref().unwrap();
            assert!(row.tail.is_none());
            assert_eq!(row.effects.len(), 1);
            assert_eq!(row.effects[0].name, "Console.stdout_write");
        }
    }
}

#[test]
fn ordinary_and_selected_method_chains_preserve_complete_return_outcomes() {
    let source = include_str!("fixtures/method_resolution/returned_chain.es");
    for source in [
        source.to_owned(),
        source.replace(".number()", "::Number.number()"),
    ] {
        let output = etas_frontend::Frontend
            .check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        let exits = summary.exits.as_ref().expect("checked call exits");
        assert!(exits.gaps.is_empty(), "{exits:?}");
        assert!(!exits.exits.is_empty());
        assert!(
            exits
                .exits
                .iter()
                .all(|exit| matches!(exit.outcome, etas_effects::CallableOutcome::Returned { .. })),
            "{exits:?}"
        );
    }
}

#[test]
fn entry_reachability_follows_only_the_selected_method_bodies() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        include_str!("fixtures/method_resolution/reachable_chain.es"),
    )));
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_eq!(checked.types.checked_method_calls.len(), 2);
    for fact in checked.types.checked_method_calls.values() {
        let MethodTarget::Source { callable, body, .. } = fact.target else {
            panic!("expected a direct checked source body");
        };
        assert!(checked.reachability.reachable_items.contains(&callable));
        assert!(checked.reachability.analysis_items.contains(&callable));
        assert_eq!(checked.hir.view().body(body).unwrap().item().id(), callable);
    }
    assert_eq!(checked.reachability.reachable_items.len(), 3);
}

#[test]
fn entry_reachability_does_not_check_unselected_implementations_of_a_generic_bound() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        include_str!("fixtures/method_resolution/reachable_bound.es"),
    )));
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let token = checked
        .types
        .symbol_types
        .iter()
        .find_map(|(symbol, fact)| {
            if checked.hir.symbols.get(*symbol)?.name != "Token" {
                return None;
            }
            match fact {
                etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                    Some(etas_types::TypeId(constructor.0))
                }
                _ => None,
            }
        })
        .unwrap();
    assert!(
        checked
            .type_store
            .iter()
            .any(|(_, ty)| *ty == etas_types::Type::Array(token)),
        "the hidden generic owner must have a closed standard-method ABI"
    );
}
