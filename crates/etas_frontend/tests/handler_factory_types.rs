use etas_frontend::{EntryPolicy, Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    Frontend.check_project(input)
}

#[test]
fn generic_handler_factory_keeps_its_checked_result_type() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
let gate: ![Gate => [] for bool] = handler { Gate.ask() => { resume true; } };
flow choose<T>(h: ![Gate => [] for T]) -> ![Gate => [] for T] {
    return h;
}
flow run() -> bool {
    return handle { perform Gate.ask() } with choose(gate);
}"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn handler_factory_generic_result_is_inferred_from_handle_context() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
flow choose<T>() -> ![Gate => [] for T] {
    return handler { Gate.ask() => { resume true; } };
}
flow run() -> bool {
    return handle { perform Gate.ask() } with choose();
}"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let binding = checked
        .types
        .generic_instantiations
        .values()
        .flat_map(|fact| &fact.type_bindings)
        .find(|(name, _)| name == "T")
        .expect("checked generic instantiation for choose");
    assert_eq!(
        checked.type_store.get(binding.1),
        Some(&etas_types::Type::Primitive(
            etas_types::PrimitiveType::Bool
        ))
    );
}

#[test]
fn handler_factory_cannot_return_a_non_handler() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
flow choose() -> bool { return true; }
flow run() -> bool { return handle { perform Gate.ask() } with choose(); }"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("handle expects a handler value")
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn unknown_handler_produced_row_is_not_treated_as_empty() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
flow run(h: ![Gate]) -> bool {
    return handle { perform Gate.ask() } with h;
}"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                etas_core::DiagnosticCode::Effect(
                    etas_core::EffectDiagnosticCode::IncompleteEffectFacts
                )
            )
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn inferred_handler_row_uses_immutable_value_evidence() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
flow run() -> bool ![Console.stdout_write] {
    let h: ![Gate] = handler {
        Gate.ask() => { perform Console.stdout_write("arm"); resume true; }
    };
    let alias: ![Gate] = h;
    return handle { perform Gate.ask() } with alias;
}"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let run = checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| {
            let etas_hir::HirItem::Flow(flow) = data else {
                return None;
            };
            (checked.hir.symbols.get(flow.symbol)?.name == "run").then_some(item)
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&run];
    assert!(summary.escaping_effects.effects.iter().any(|effect| matches!(effect,
        etas_effects::Effect::Action(action)
            if action.tag == etas_effects::CONSOLE_TAG && action.action == etas_effects::CONSOLE_STDOUT_WRITE_ACTION
    )), "{summary:?}");
}

#[test]
fn inferred_handler_row_does_not_reuse_a_mutable_initializer() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
flow run() -> bool {
    var h: ![Gate] = handler { Gate.ask() => { resume true; } };
    return handle { perform Gate.ask() } with h;
}"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.code,
            etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts
            )
        )),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn handler_factory_finish_value_must_match_handle_result() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> i32; }
let gate: ![Gate => [] for bool] = handler { Gate.ask() => { finish true; } };
flow choose(h: ![Gate => [] for bool]) -> ![Gate => [] for bool] { return h; }
flow run() -> i32 { return handle { perform Gate.ask() } with choose(gate); }"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::FinishTypeMismatch)
            )
        }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn handler_expression_early_return_is_not_replaced_by_body_return() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
let gate: ![Gate => [] for bool] = handler { Gate.ask() => { resume true; } };
flow run(flag: bool) -> bool {
    let result = handle { perform Gate.ask() } with (if flag { return false; } else { gate });
    perform Console.stdout_write("after");
    return result;
}"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let run = checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| {
            let etas_hir::HirItem::Flow(flow) = data else {
                return None;
            };
            (checked.hir.symbols.get(flow.symbol)?.name == "run").then_some(item)
        })
        .unwrap();
    let exits = checked.effects.item_effects[&run].exits.as_ref().unwrap();
    let early = exits
        .exits
        .iter()
        .find(|exit| {
            matches!(
                exit.outcome,
                etas_effects::CallableOutcome::Returned {
                    observation: etas_effects::ReturnObservation::Bool(false),
                    ..
                }
            )
        })
        .unwrap_or_else(|| panic!("{exits:?}"));
    assert_eq!(
        early.trace,
        etas_effects::ActionTraceDomain::Empty,
        "{exits:?}"
    );
}

#[test]
fn nonreturning_handler_factory_does_not_execute_the_handled_body() {
    let output = check(
        r#"module app;
effect Gate { action ask() -> bool; }
flow choose() -> ![Gate => [] for bool] ![Error<i32>] { return perform Error<i32>.raise(1); }
flow run() -> bool ![Error<i32>] {
    return handle { perform Gate.ask() } with choose();
}"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let run = checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| {
            let etas_hir::HirItem::Flow(flow) = data else {
                return None;
            };
            (checked.hir.symbols.get(flow.symbol)?.name == "run").then_some(item)
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&run];
    let choose = checked
        .hir
        .items
        .iter()
        .find_map(|(item, data)| {
            let etas_hir::HirItem::Flow(flow) = data else {
                return None;
            };
            (checked.hir.symbols.get(flow.symbol)?.name == "choose").then_some(item)
        })
        .unwrap();
    let gate = checked.effect_registry.tag_by_name("Gate").unwrap();
    assert!(
        !summary
            .requested_actions
            .effects
            .iter()
            .any(|effect| matches!(effect,
                etas_effects::Effect::Action(action) if action.tag == gate
            )),
        "run={summary:?} choose={:?}",
        checked.effects.item_effects[&choose]
    );
    let exits = summary.exits.as_ref().unwrap();
    assert!(
        exits.exits.iter().all(|exit| matches!(
            exit.outcome,
            etas_effects::CallableOutcome::Raised { .. } | etas_effects::CallableOutcome::NonReturn
        )),
        "{exits:?}"
    );
    assert!(!exits.exits.is_empty(), "{exits:?}");
}
