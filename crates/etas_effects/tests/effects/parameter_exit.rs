use super::*;

#[test]
fn parameter_exit_keeps_nested_handler_sources_in_unwind_order() {
    let source = r#"
effect Inner { action ping() -> unit; }
effect Outer { action pong() -> unit; }
flow relay(f: () -> unit ![Inner.ping, Outer.pong]) -> unit ![] {
    handle {
        handle { f(); } with { Inner.ping() => { resume; } };
    } with { Outer.pong() => { resume; } };
    return;
}

"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "relay")]
        .exits
        .as_ref()
        .unwrap();
    let [exit] = exits.parameter_exits.as_slice() else {
        panic!("expected one callback exit: {exits:?}");
    };
    let handlers = exit.enclosing_handlers.as_ref().unwrap();
    assert_eq!(handlers.len(), 2);
    let spans = handlers
        .iter()
        .map(|handler| {
            let span = hir.exprs.get(*handler).unwrap().span(&hir.blocks);
            &source[span.range.start.to_usize()..span.range.end.to_usize()]
        })
        .collect::<Vec<_>>();
    assert!(spans[0].contains("Inner.ping"), "{spans:?}");
    assert!(spans[1].contains("Outer.pong"), "{spans:?}");
}

#[test]
fn parameter_exit_carries_handler_scope_across_forwarding_call() {
    let source = r#"
effect Inner { action ping() -> unit; }
effect Outer { action pong() -> unit; }
flow invoke(f: () -> unit ![Inner.ping, Outer.pong]) -> unit ![Outer.pong] {
    handle { f(); } with { Inner.ping() => { resume; } };
    return;
}
flow relay(f: () -> unit ![Inner.ping, Outer.pong]) -> unit ![] {
    handle { invoke(f); } with { Outer.pong() => { resume; } };
    return;
}
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "relay")]
        .exits
        .as_ref()
        .unwrap();
    let [exit] = exits.parameter_exits.as_slice() else {
        panic!("expected one forwarded callback exit: {exits:?}");
    };
    let handlers = exit.enclosing_handlers.as_ref().unwrap();
    assert_eq!(handlers.len(), 2);
    let spans = handlers
        .iter()
        .map(|handler| {
            let span = hir.exprs.get(*handler).unwrap().span(&hir.blocks);
            &source[span.range.start.to_usize()..span.range.end.to_usize()]
        })
        .collect::<Vec<_>>();
    assert!(spans[0].contains("Inner.ping"), "{spans:?}");
    assert!(spans[1].contains("Outer.pong"), "{spans:?}");
}

#[test]
fn recursive_forwarding_with_a_handler_converges_without_forging_a_scope() {
    let source = r#"
effect Inner { action ping() -> unit; }
flow recurse(f: () -> unit ![Inner.ping], stop: bool) -> unit ![Inner.ping] {
    if stop { f(); return; }
    handle { recurse(f, true); } with { Inner.ping() => { resume; } };
    return;
}
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "recurse")];
    let exits = summary.exits.as_ref().unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Callee(_))),
        "recursive call control must remain explicitly unproved: {exits:?}"
    );
}

#[test]
fn callback_requests_handled_by_local_handler_do_not_become_default_actions() {
    let source = r#"
import std.io.println;
flow invoke(f: () -> unit ![]) -> unit ![] {
    handle { f(); } with { Console.stdout_write(_) => { resume; } };
    return;
}
flow write() -> unit ![] {
    println("hello")?;
    return;
}
flow main() -> unit ![] { invoke(write); return; }
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let console = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    let write = &output.facts.item_effects[&flow_item(&hir, "write")];
    assert!(
        write.default_actions.effects.contains(&console),
        "{write:?}"
    );
    assert!(summary.escaping_effects.is_empty(), "{summary:?}");
    assert!(
        summary.requested_actions.effects.contains(&console),
        "{summary:?}"
    );
    assert!(
        !summary.default_actions.effects.contains(&console),
        "{summary:?}"
    );
    assert!(
        summary.action_trace.action_set().contains(&console),
        "{summary:?}"
    );
}

#[test]
fn immutable_handler_value_intercepts_callback_default_action() {
    let source = r#"
import std.io.println;
flow invoke(f: () -> unit ![]) -> unit ![] {
    let local_handler = handler { Console.stdout_write(_) => { resume; } };
    handle { f(); } with local_handler;
    return;
}
flow write() -> unit ![] { println("hello")?; return; }
flow main() -> unit ![] { invoke(write); return; }
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let console = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&console),
        "{summary:?}"
    );
    assert!(
        !summary.default_actions.effects.contains(&console),
        "{summary:?}"
    );
}

#[test]
fn refutable_callback_handler_keeps_possible_default_action() {
    let source = r#"
import std.io.println;
flow invoke(f: () -> unit ![]) -> unit ![] {
    handle { f(); } with { Console.stdout_write("other") => { resume; } };
    return;
}
flow write() -> unit ![] { println("hello")?; return; }
flow main() -> unit ![] { invoke(write); return; }
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let console = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.default_actions.effects.contains(&console),
        "{summary:?}"
    );
    assert!(
        summary.requested_actions.effects.contains(&console),
        "{summary:?}"
    );
    assert!(
        matches!(
            &summary.handler_requests,
            etas_effects::RequestInstantiationSummary::Available { actions, .. }
                if actions.iter().any(|request| request.action == console)
        ),
        "{summary:?}"
    );
}

#[test]
fn callback_outside_handler_keeps_possible_default_action() {
    let source = r#"
import std.io.println;
flow invoke(f: () -> unit ![], handle_it: bool) -> unit ![] {
    if handle_it {
        handle { f(); } with { Console.stdout_write(_) => { resume; } };
    } else {
        f();
    }
    return;
}
flow write() -> unit ![] { println("hello")?; return; }
flow main() -> unit ![] { invoke(write, true); return; }
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let console = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.default_actions.effects.contains(&console),
        "{summary:?}"
    );
}

#[test]
fn parameter_exit_facts_retain_distinct_checked_call_arguments() {
    let source = "flow relay(f: (i32) -> unit ![]) -> unit ![] { f(0); f(1); return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "relay")];
    let exits = summary.exits.as_ref().unwrap();
    assert_eq!(exits.parameter_exits.len(), 2);
    let mut arguments = exits
        .parameter_exits
        .iter()
        .map(|exit| {
            let [argument] = exit.arguments.as_ref().unwrap().as_slice() else {
                panic!("expected one checked argument")
            };
            assert!(matches!(
                types.store.get(argument.ty),
                Some(etas_types::Type::Primitive(etas_types::PrimitiveType::I32))
            ));
            assert!(matches!(
                argument.value,
                Some(etas_effects::ParameterArgumentValue::Integer {
                    negative: false,
                    magnitude: 0 | 1
                })
            ));
            let range = argument.source.span().range;
            &source[range.start.to_usize()..range.end.to_usize()]
        })
        .collect::<Vec<_>>();
    arguments.sort_unstable();
    assert_eq!(arguments, ["0", "1"]);
    assert_eq!(
        exits
            .gaps
            .iter()
            .filter(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::ParameterCall { .. }))
            .count(),
        2
    );
}

#[test]
fn parameter_exit_records_checked_negative_integer_boundaries() {
    let source = "flow relay(f: (i8) -> unit ![]) -> unit ![] { f(-128); return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "relay")]
        .exits
        .as_ref()
        .unwrap();
    let [exit] = exits.parameter_exits.as_slice() else {
        panic!("one callback exit")
    };
    let [argument] = exit.arguments.as_ref().unwrap().as_slice() else {
        panic!("one argument")
    };
    assert_eq!(
        argument.value,
        Some(etas_effects::ParameterArgumentValue::Integer {
            negative: true,
            magnitude: 128
        })
    );
    assert!(
        exits
            .gaps
            .iter()
            .all(|gap| !matches!(gap, etas_effects::OutcomeAnalysisGap::Operation(_)))
    );
}

#[test]
fn parameter_exit_records_literals_and_unmodified_checked_inputs() {
    let source = "flow relay(value: bool, f: (bool) -> unit ![]) -> unit ![] { f(true); f(false); f(value); return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "relay")]
        .exits
        .as_ref()
        .unwrap();
    let mut facts = exits
        .parameter_exits
        .iter()
        .map(|exit| {
            let [argument] = exit.arguments.as_ref().unwrap().as_slice() else {
                panic!("one argument")
            };
            let range = argument.source.span().range;
            (
                &source[range.start.to_usize()..range.end.to_usize()],
                argument.value.clone(),
            )
        })
        .collect::<Vec<_>>();
    facts.sort_by_key(|(source, _)| *source);
    assert_eq!(
        facts,
        vec![
            (
                "false",
                Some(etas_effects::ParameterArgumentValue::Bool(false))
            ),
            (
                "true",
                Some(etas_effects::ParameterArgumentValue::Bool(true))
            ),
            (
                "value",
                Some(etas_effects::ParameterArgumentValue::Input { position: 0 })
            ),
        ]
    );
}

#[test]
fn parameter_exit_does_not_treat_a_reassigned_parameter_as_its_entry_value() {
    let source = "flow relay(value: bool, f: (bool) -> unit ![]) -> unit ![] { value = false; f(value); return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "relay")]
        .exits
        .as_ref()
        .unwrap();
    let [exit] = exits.parameter_exits.as_slice() else {
        panic!("one callback exit")
    };
    let [argument] = exit.arguments.as_ref().unwrap().as_slice() else {
        panic!("one argument")
    };
    assert_eq!(argument.value, None);
}

#[test]
fn parameter_exit_does_not_treat_a_captured_write_as_unchanged() {
    let source = "flow relay(value: bool, f: (bool) -> unit ![]) -> unit ![] { let change = () => { value = false; return; }; change(); f(value); return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "relay")]
        .exits
        .as_ref()
        .unwrap();
    let [exit] = exits.parameter_exits.as_slice() else {
        panic!("one callback exit")
    };
    let [argument] = exit.arguments.as_ref().unwrap().as_slice() else {
        panic!("one argument")
    };
    assert_eq!(argument.value, None);
}

#[test]
fn callback_effects_do_not_become_independent_body_determinism() {
    let source = "flow relay(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { f(); return; } flow own(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { f(); perform Console.stdout_write(\"own\"); return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let relay = &output.facts.item_effects[&flow_item(&hir, "relay")];
    let own = &output.facts.item_effects[&flow_item(&hir, "own")];
    assert_eq!(
        relay.determinism,
        etas_effects::Determinism::RuntimeMediated
    );
    assert_eq!(
        relay.independent_determinism,
        Some(etas_effects::Determinism::Deterministic)
    );
    assert_eq!(
        own.independent_determinism,
        Some(etas_effects::Determinism::RuntimeMediated)
    );
}

#[test]
fn effectful_callback_facts_record_local_handler_scope() {
    let source = "flow plain(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { f(); return; } flow wrapped(f: () -> unit ![Console.stdout_write]) -> unit ![] { handle { f(); } with { Console.stdout_write(_) => { resume; } }; return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    for (name, expected) in [("plain", true), ("wrapped", false)] {
        let exits = output.facts.item_effects[&flow_item(&hir, name)]
            .exits
            .as_ref()
            .unwrap();
        let [exit] = exits.parameter_exits.as_slice() else {
            panic!("{name}: {exits:?}")
        };
        assert_eq!(exit.handler_free, Some(expected), "{name}");
    }
}

#[test]
fn effectful_callback_facts_record_disjoint_handler_row() {
    let source = "flow wrapped(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { handle { f(); } with { Error<i32>.raise(_) => { finish (); } }; return; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "wrapped")]
        .exits
        .as_ref()
        .unwrap();
    let [exit] = exits.parameter_exits.as_slice() else {
        panic!("{exits:?}")
    };
    assert_eq!(exit.handler_free, Some(false));
    assert!(
        exit.enclosing_handled
            .as_ref()
            .is_some_and(|row| !row.effects.is_empty()),
        "{exits:?}"
    );
    assert_eq!(exits.handler_gaps.len(), 1, "{exits:?}");
}

#[test]
fn implicit_unit_return_retains_handler_gap_evidence() {
    let source = "flow wrapped(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { handle { f(); } with { Error<i32>.raise(_) => { finish (); } }; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "wrapped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Handler(_)))
    );
    assert_eq!(exits.handler_gaps.len(), 1, "{exits:?}");
}

#[test]
fn helper_preserves_forwarded_callback_outcome_and_handler_evidence() {
    let source = "flow helper(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { handle { f(); } with { Error<i32>.raise(_) => { finish (); } }; } flow wrapper(f: () -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] { perform Console.stdout_write(\"before\"); helper(f); }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "wrapper")]
        .exits
        .as_ref()
        .unwrap();
    assert_eq!(exits.parameter_exits.len(), 1, "{exits:?}");
    assert_eq!(exits.handler_gaps.len(), 1, "{exits:?}");
    let exit = &exits.parameter_exits[0];
    let etas_effects::ParameterCallTarget::Native(symbol) = exit.target else {
        panic!("{exit:?}")
    };
    assert!(
        matches!(hir.symbols.get(symbol).map(|data| &data.def), Some(etas_hir::SymbolDef::Param { owner, .. }) if *owner == flow_item(&hir, "wrapper"))
    );
    assert!(
        matches!(&exit.prefix, etas_effects::ActionTraceDomain::Event(event)
        if event.action == etas_effects::Effect::Action(etas_effects::ActionRef {
            tag: etas_effects::CONSOLE_TAG,
            action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
        }))
    );
    assert!(
        exits
            .gaps
            .contains(&etas_effects::OutcomeAnalysisGap::ParameterCall {
                target: exit.target,
                output: exit.output,
                source: exit.source.clone(),
            })
    );
}

#[test]
fn forwarded_callback_arguments_preserve_checked_values_and_unknowns() {
    let source = "flow constant(f: (bool) -> unit ![]) -> unit ![] { f(true); } flow helper(f: (bool) -> unit ![], value: bool) -> unit ![] { f(value); } flow direct(f: (bool) -> unit ![]) -> unit ![] { constant(f); } flow literal(f: (bool) -> unit ![]) -> unit ![] { helper(f, true); } flow input(f: (bool) -> unit ![], value: bool) -> unit ![] { helper(f, value); } flow unknown(f: (bool) -> unit ![], value: bool) -> unit ![] { helper(f, !value); }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    for (name, expected) in [
        (
            "direct",
            Some(etas_effects::ParameterArgumentValue::Bool(true)),
        ),
        (
            "literal",
            Some(etas_effects::ParameterArgumentValue::Bool(true)),
        ),
        (
            "input",
            Some(etas_effects::ParameterArgumentValue::Input { position: 1 }),
        ),
        ("unknown", None),
    ] {
        let exits = output.facts.item_effects[&flow_item(&hir, name)]
            .exits
            .as_ref()
            .unwrap();
        let [exit] = exits.parameter_exits.as_slice() else {
            panic!("{name}: {exits:?}")
        };
        let [argument] = exit.arguments.as_deref().unwrap() else {
            panic!("{name}: {exit:?}")
        };
        assert_eq!(argument.value, expected, "{name}: {exit:?}");
    }
}

#[test]
fn handler_arm_outcomes_distinguish_resume_and_finish_with_trace() {
    let source = "effect Gate { action ask() -> bool; action signal() -> unit; } flow resume_bool() -> bool ![Console.stdout_write] { return handle { perform Gate.ask() } with { Gate.ask() => { perform Console.stdout_write(\"arm\"); resume true; } }; } flow resume_unit() -> unit ![] { handle { perform Gate.signal(); } with { Gate.signal() => { resume; } }; } flow finish_bool() -> bool ![] { return handle { perform Gate.ask() } with { Gate.ask() => { finish false; } }; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    for name in ["resume_bool", "resume_unit", "finish_bool"] {
        let owner = flow_item(&hir, name);
        let (unit, summary) = output.facts.unit_effects.iter().find(|(unit, _)|
            matches!(unit, etas_effects::EffectUnit::HandlerArm { owner: item, .. } if *item == owner)
        ).unwrap_or_else(|| panic!("missing handler arm for {name}"));
        assert_eq!(
            output
                .facts
                .handler_values
                .values()
                .flat_map(|handler| &handler.arms)
                .filter(|arm| arm.unit == *unit)
                .count(),
            1,
            "{name}: handler evidence must identify its solved unit"
        );
        let exits = summary.exits.as_ref().unwrap();
        if name == "resume_bool" {
            assert!(
                exits
                    .gaps
                    .iter()
                    .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::ExternalCall(_))),
                "{exits:?}"
            );
        } else {
            assert!(exits.gaps.is_empty(), "{name}: {exits:?}");
        }
        let [exit] = exits.exits.as_slice() else {
            panic!("{name}: {exits:?}")
        };
        match name {
            "resume_bool" => {
                assert!(matches!(
                    exit.outcome,
                    etas_effects::CallableOutcome::HandlerResumed {
                        observation: etas_effects::ReturnObservation::Bool(true),
                        ..
                    }
                ));
                assert!(
                    matches!(&exit.trace, etas_effects::ActionTraceDomain::Event(event)
                    if event.action == etas_effects::Effect::Action(etas_effects::ActionRef { tag: etas_effects::CONSOLE_TAG, action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION }))
                );
            }
            "resume_unit" => assert!(matches!(
                exit.outcome,
                etas_effects::CallableOutcome::HandlerResumed {
                    observation: etas_effects::ReturnObservation::Unit,
                    ..
                }
            )),
            "finish_bool" => assert!(matches!(
                exit.outcome,
                etas_effects::CallableOutcome::HandlerFinished {
                    observation: etas_effects::ReturnObservation::Bool(false),
                    ..
                }
            )),
            _ => unreachable!(),
        }
    }
}

#[test]
fn handler_factory_effect_precedes_handled_body_in_callable_trace() {
    let source = r#"
effect Gate { action ask() -> bool; }
let gate: ![Gate => []] = handler { Gate.ask() => { resume true; } };
flow choose(h: ![Gate => []]) -> ![Gate => []] ![Console.stdout_write] {
    perform Console.stdout_write("factory");
    return h;
}
flow run() -> bool ![Console.stdout_write] {
    perform Console.stdout_write("before");
    return handle { perform Gate.ask() } with choose(gate);
}
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_top_level_items(&hir, etas_types::check_program(&hir));
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "run")];
    let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!("expected ordered handler factory and body actions: {summary:?}");
    };
    assert_eq!(events.len(), 3, "{events:?}");
    let console = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| *effect == console)
    );
    assert!(
        matches!(&events[0], etas_effects::ActionTraceDomain::Event(event)
        if event.action == console),
        "{events:?}"
    );
    assert!(
        matches!(&events[1], etas_effects::ActionTraceDomain::Event(event)
        if event.action == console),
        "{events:?}"
    );
    assert!(
        matches!(&events[2], etas_effects::ActionTraceDomain::Event(event)
        if event.action != console),
        "{events:?}"
    );
    let exits = summary.exits.as_ref().unwrap();
    let [exit] = exits.exits.as_slice() else {
        panic!("{exits:?}")
    };
    assert_eq!(exit.trace, summary.action_trace);
}

#[test]
fn handler_resume_preserves_callable_source_identity() {
    let source = "effect Gate { action select() -> (() -> i32 ![]); } flow value() -> i32 ![] { return 1; } flow run() -> (() -> i32 ![]) ![] { return handle { perform Gate.select() } with { Gate.select() => { resume value; } }; }";
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let owner = flow_item(&hir, "run");
    let (_, summary) = output.facts.unit_effects.iter().find(|(unit, _)|
        matches!(unit, etas_effects::EffectUnit::HandlerArm { owner: item, .. } if *item == owner)
    ).unwrap();
    let exits = summary.exits.as_ref().unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    let [exit] = exits.exits.as_slice() else {
        panic!("{exits:?}")
    };
    assert!(
        matches!(&exit.outcome,
            etas_effects::CallableOutcome::HandlerResumed {
                observation: etas_effects::ReturnObservation::CallableSources(sources), ..
            } if sources.contains(&etas_effects::CallableReturnSource::SourceItem(flow_item(&hir, "value")))
        ),
        "{exit:?}"
    );
}
