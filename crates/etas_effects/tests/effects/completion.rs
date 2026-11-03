use super::*;

#[test]
fn resuming_handler_actions_keep_their_execution_position_and_multiplicity() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
effect Gate { action check() -> unit; action send() -> unit; }
effect Audit { action note() -> unit; }
flow complete() -> unit {
    handle { perform Gate.check(); perform Gate.check(); perform Gate.send(); } with {
        Gate.check() => {
            handle { perform Audit.note(); } with { Audit.note() => { resume (); } };
            resume ();
        }
        Gate.send() => { resume (); }
    };
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "complete")];
    let exits = summary.exits.as_ref().unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    let mut pending = vec![&summary.action_trace];
    let mut events = Vec::new();
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Empty => {}
            ActionTraceDomain::Event(event) => events.push(event.action.clone()),
            ActionTraceDomain::Seq(parts) => pending.extend(parts.iter().rev()),
            other => panic!("expected exact sequential handler trace, got {other:?}"),
        }
    }
    let action = |owner: &str, name: &str| {
        output
            .facts
            .performed_actions
            .values()
            .find(|fact| fact.effect_segments == [owner] && fact.action == name)
            .unwrap()
            .invocation
            .clone()
    };
    assert_eq!(
        events,
        [
            action("Gate", "check"),
            action("Audit", "note"),
            action("Gate", "check"),
            action("Audit", "note"),
            action("Gate", "send")
        ]
    );
    assert!(summary.escaping_effects.is_empty());
    assert_eq!(summary.requested_actions.effects.len(), 3);
}

#[test]
fn exhaustive_total_resuming_handlers_complete_owned_perform_outcomes() {
    for arm in [
        "resume ();",
        "let value = (); resume value;",
        "let value = if flag { () } else { () }; resume value;",
        "let value = helper(); resume value;",
    ] {
        let source = format!(
            "module app; effect Gate {{ action check() -> unit; }} flow helper() -> unit {{ return; }} flow complete(flag: bool) -> unit {{ handle {{ perform Gate.check(); }} with {{ Gate.check() => {{ {arm} }} }}; return; }}"
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let output = check_program(&hir, &types);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let summary = &output.facts.item_effects[&flow_item(&hir, "complete")];
        let exits = summary.exits.as_ref().unwrap();
        assert!(exits.gaps.is_empty(), "{arm}: {exits:?}");
        assert!(!exits.exits.is_empty(), "{arm}: {exits:?}");
        assert!(summary.escaping_effects.is_empty());
        assert_eq!(summary.requested_actions.effects.len(), 1);
        assert_eq!(
            summary.action_trace.action_set(),
            summary.requested_actions.effects
        );
    }
}

#[test]
fn immutable_handler_bindings_preserve_total_resumption_evidence() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        "module app; effect Gate { action check() -> unit; } flow complete() -> unit { let selected = handler { Gate.check() => { let value = (); resume value; } }; handle { perform Gate.check(); perform Gate.check(); } with selected; return; }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "complete")];
    let exits = summary.exits.as_ref().unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    assert!(
        matches!(summary.action_trace, ActionTraceDomain::Seq(ref events) if events.len() == 2)
    );
}

#[test]
fn resumption_completion_does_not_discharge_unproved_or_foreign_control() {
    for (case, body) in [
        (
            "finish",
            "handle { perform Gate.check(true); } with { Gate.check(_) => { finish (); } };",
        ),
        (
            "partial pattern",
            "handle { perform Gate.check(true); } with { Gate.check(true) => { resume (); } };",
        ),
        (
            "earlier finish arm",
            "handle { perform Gate.check(true); } with { Gate.check(true) => { finish (); } Gate.check(_) => { resume (); } };",
        ),
        (
            "unproved arm",
            "handle { perform Gate.check(true); } with { Gate.check(_) => { let next = 1 + 2; resume (); } };",
        ),
        (
            "arm action",
            "handle { perform Gate.check(true); } with { Gate.check(_) => { perform Gate.extra(); resume (); } };",
        ),
        (
            "prior request",
            "perform Gate.check(true); handle { perform Gate.check(true); } with { Gate.check(_) => { resume (); } };",
        ),
        (
            "unknown callee",
            "handle { helper(); perform Gate.check(true); } with { Gate.check(_) => { resume (); } };",
        ),
        (
            "nested finish",
            "handle { handle { perform Gate.check(true); } with { Gate.check(_) => { finish (); } }; } with { Gate.check(_) => { resume (); } };",
        ),
    ] {
        let source = format!(
            "module app; effect Gate {{ action check(value: bool) -> unit; action extra() -> unit; }} flow helper() -> unit ![Gate.extra] {{ perform Gate.extra(); return; }} flow uncertain() -> unit ![Gate.check, Gate.extra] {{ {body} return; }}"
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
        assert!(
            parsed.diagnostics.is_empty(),
            "{case}: {:?}",
            parsed.diagnostics
        );
        let hir = lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(
            types.diagnostics.is_empty(),
            "{case}: {:?}",
            types.diagnostics
        );
        let output = check_program(&hir, &types);
        assert!(
            output.diagnostics.is_empty(),
            "{case}: {:?}",
            output.diagnostics
        );
        let exits = output.facts.item_effects[&flow_item(&hir, "uncertain")]
            .exits
            .as_ref()
            .unwrap();
        assert!(!exits.gaps.is_empty(), "{case}: {exits:?}");
    }
}

#[test]
fn integer_identity_operations_have_checked_total_outcomes() {
    for ty in [
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    ] {
        let source = format!(
            "module app; flow safe(x: {ty}) -> {ty} {{ let a = x + 0; let b = 0 + x; let c = x - 0; let d = x * 0; let e = 0 * x; let f = x * 1; return 1 * x; }}"
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source.as_str()));
        assert!(
            parsed.diagnostics.is_empty(),
            "{ty}: {:?}",
            parsed.diagnostics
        );
        let hir = lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(
            types.diagnostics.is_empty(),
            "{ty}: {:?}",
            types.diagnostics
        );
        let output = check_program(&hir, &types);
        let exits = output.facts.item_effects[&flow_item(&hir, "safe")]
            .exits
            .as_ref()
            .unwrap();
        assert!(exits.gaps.is_empty(), "{ty}: {exits:?}");
        assert!(!exits.exits.is_empty(), "{ty}: {exits:?}");
    }
}

#[test]
fn unchecked_integer_and_float_operations_remain_unproved() {
    for (ty, expression) in [
        ("i32", "x + 1"),
        ("i32", "0 - x"),
        ("i32", "x * 2"),
        ("i32", "(x + 1) * 0"),
        ("i32", "x / 0"),
        ("u8", "x - 1"),
        ("f32", "x + 0.0"),
    ] {
        let source =
            format!("module app; flow uncertain(x: {ty}) -> {ty} {{ return {expression}; }}");
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source.as_str()));
        assert!(
            parsed.diagnostics.is_empty(),
            "{expression}: {:?}",
            parsed.diagnostics
        );
        let hir = lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(
            types.diagnostics.is_empty(),
            "{expression}: {:?}",
            types.diagnostics
        );
        let output = check_program(&hir, &types);
        let exits = output.facts.item_effects[&flow_item(&hir, "uncertain")]
            .exits
            .as_ref()
            .unwrap();
        assert!(
            exits
                .gaps
                .iter()
                .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Operation(_))),
            "{expression}: {exits:?}"
        );
    }
}

#[test]
fn checked_limit_constructors_have_complete_outcomes() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow limits() -> unit {
    let iterations = Iterations(8);
    let tokens = Tokens(9);
    let context = ContextTokens(10);
    let attempts = Attempts(11);
    return;
}

"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let exits = output.facts.item_effects[&flow_item(&hir, "limits")]
        .exits
        .as_ref()
        .unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    assert!(!exits.exits.is_empty(), "{exits:?}");
}

#[test]
fn checked_bounded_for_preserves_complete_pure_loop_outcomes() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow looped(values: Array<bool>) -> unit {
    for value in values limit Iterations(8) { let copy = value; }
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    assert!(!exits.exits.is_empty(), "{exits:?}");
    assert!(
        !exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::ExternalCall(_))),
        "{exits:?}"
    );
}

#[test]
fn checked_bounded_for_does_not_hide_unproved_body_operations() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow looped(values: Array<i32>) -> unit {
    for value in values limit Iterations(3) { let next = value + 1; }
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Operation(_))),
        "{exits:?}"
    );
}

#[test]
fn checked_bounded_for_keeps_break_outcomes_unproven() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow looped(values: Array<bool>) -> unit {
    for value in values limit Iterations(3) { if value { break; } }
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Loop(_))),
        "{exits:?}"
    );
}

#[test]
fn generic_iteration_element_does_not_prove_callable_absence() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        "module app; flow looped<T>(values: Array<T>) -> unit { for value in values limit Iterations(3) { let copy = value; } return; }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Loop(_))),
        "{exits:?}"
    );
}

#[test]
fn while_condition_action_is_not_misrepresented_as_a_single_request() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        "module app; effect Gate { action ask() -> bool; } flow looped() -> unit ![Gate.ask] { while perform Gate.ask() limit Iterations(2) { } return; }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "looped")];
    assert!(
        matches!(summary.action_trace, ActionTraceDomain::Widened { .. }),
        "{:?}",
        summary.action_trace
    );
    assert!(!summary.action_trace.action_set().is_empty());
    assert!(
        summary
            .exits
            .as_ref()
            .unwrap()
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Loop(_)))
    );
}

#[test]
fn checked_boolean_operations_and_count_queries_preserve_complete_return_summaries() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow logic(a: bool, b: bool) -> bool { return !(a == b) && (a != b || a); }
flow size(values: Array<i32>) -> usize { return values.len(); }
flow emptiness(values: Array<i32>) -> bool { return values.is_empty(); }
flow arithmetic(value: i8) -> i8 { return value + 1; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for (item, declaration) in hir.items.iter() {
        let HirItem::Flow(flow) = declaration else {
            continue;
        };
        let name = &hir.symbols.get(flow.symbol).unwrap().name;
        let exits = output.facts.item_effects[&item].exits.as_ref().unwrap();
        if name == "arithmetic" {
            assert!(
                exits
                    .gaps
                    .iter()
                    .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Operation(_))),
                "{exits:?}"
            );
        } else {
            assert!(exits.gaps.is_empty(), "{name}: {exits:?}");
            assert!(!exits.exits.is_empty(), "{name}: missing return summary");
        }
    }
}
