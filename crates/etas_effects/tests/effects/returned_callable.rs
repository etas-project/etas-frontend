use super::*;

#[test]
fn chained_callable_invocation_does_not_evaluate_the_factory_twice() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow write() -> unit ![] { println("callback")?; return; }
flow factory() -> (() -> unit ![]) { println("factory")?; return write; }
flow main() -> unit { factory()(); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let mut pending = vec![&summary.action_trace];
    let mut events = 0;
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Empty => {}
            ActionTraceDomain::Seq(children) => pending.extend(children),
            ActionTraceDomain::Event(_) => events += 1,
            other => panic!("unexpected factory trace: {other:?}"),
        }
    }
    assert_eq!(
        events, 2,
        "the factory and callback each run once: {summary:?}"
    );
}

#[test]
fn chained_first_class_calls_use_solved_callable_reference_dependencies() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow select<T, effect E>(callback: (T) -> T ![E]) -> ((T) -> T ![E]) ![] { return callback; }
flow fail<T>(value: T) -> T ![Error<T>] { return perform Error<T>.raise(value); }
flow main() -> i32 ![Error<i32>] { return select(fail)(2); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let escaping = summary.escaping_effects.effects.iter().collect::<Vec<_>>();
    assert!(
        matches!(escaping.as_slice(), [Effect::Error(ty)] if types.store.get(*ty) == Some(&etas_types::Type::Primitive(etas_types::PrimitiveType::I32))),
        "{summary:?}"
    );
    assert!(summary.requested_actions.is_empty());
}

#[test]
fn instantiated_return_sources_preserve_handled_actions_and_trace_multiplicity() {
    for quiet in [false, true] {
        let source = format!(
            r#"
module app;
import std.io.println;
spec Quiet: trace = -Console.stdout_write;
flow write<T>(value: T) -> T ![] {{ println("callback")?; return value; }}
flow factory() -> ((i32) -> i32 ![]) {{ return write; }}
flow main() -> i32 {} {{
    let callback = factory();
    callback(1);
    return callback(2);
}}
"#,
            if quiet { "~ Quiet" } else { "" }
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let mut types = etas_types::check_program(&hir);
        add_std_symbol_type_facts(&hir, &mut types);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let output = check_program(&hir, &types);
        if quiet {
            assert!(
                output.diagnostics.iter().any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
                "{:?}",
                output.diagnostics
            );
            assert!(
                !output.diagnostics.iter().any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
                "{:?}",
                output.diagnostics
            );
        } else {
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        }
        let factory = &output.facts.item_effects[&flow_item(&hir, "factory")];
        assert!(factory.requested_actions.is_empty());
        let stdout = Effect::Action(ActionRef {
            tag: CONSOLE_TAG,
            action: CONSOLE_STDOUT_WRITE_ACTION,
        });
        let main = &output.facts.item_effects[&flow_item(&hir, "main")];
        assert!(main.escaping_effects.is_empty());
        assert!(main.requested_actions.effects.contains(&stdout));
        assert!(main.default_actions.effects.contains(&stdout));
        let mut pending = vec![&main.action_trace];
        let mut calls = 0;
        while let Some(trace) = pending.pop() {
            match trace {
                ActionTraceDomain::Event(event) if event.action == stdout => calls += 1,
                ActionTraceDomain::Seq(children) => pending.extend(children),
                ActionTraceDomain::Empty => {}
                other => panic!("unexpected instantiated callable trace: {other:?}"),
            }
        }
        assert_eq!(calls, 2);
    }
}

#[test]
fn nullary_payload_facts_survive_wrapping_and_record_shorthand() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
type Node = { callback: Option<() -> i32 ![]>, next: Option<Node> };
flow main() -> unit {
    let empty = Node { callback = None, next = None };
    let root = Node { callback = None, next = Some(empty) };
    let wrapped = { root };
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
    assert!(
        output.facts.item_effects[&flow_item(&hir, "main")]
            .requested_actions
            .is_empty()
    );
}

#[test]
fn a_known_callback_in_another_field_cannot_hide_an_unknown_callable_payload() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
type Callbacks = { first: () -> unit ![], second: () -> unit ![] };
flow known() -> unit ![] { return; }
flow main(input: Callbacks) -> unit {
    let combined = Callbacks { first = known, second = input.second };
    let selected = combined.second;
    selected();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn a_returned_global_record_preserves_handled_callback_requests_and_trace_multiplicity() {
    for quiet in [false, true] {
        let source = format!(
            r#"
module app;
import std.io.println;
spec Quiet: trace = -Console.stdout_write;
flow write(value: i32) -> i32 ![] {{ println("callback")?; return value; }}
let action = write;
type Callbacks = {{ next: (i32) -> i32 ![] }};
let callbacks = Callbacks {{ next = action }};
flow factory() -> Callbacks {{ return callbacks; }}
flow main() -> i32 {} {{
    let selected = factory();
    let action = selected.next;
    action(1);
    return action(2);
}}
"#,
            if quiet { "~ Quiet" } else { "" }
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let mut types = etas_types::check_program(&hir);
        add_std_symbol_type_facts(&hir, &mut types);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let output = check_program(&hir, &types);
        if quiet {
            assert!(
                output.diagnostics.iter().any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
                "{:?}",
                output.diagnostics
            );
            assert!(
                !output.diagnostics.iter().any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
                "{:?}",
                output.diagnostics
            );
        } else {
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        }
        let factory = &output.facts.item_effects[&flow_item(&hir, "factory")];
        assert!(factory.requested_actions.is_empty());
        let main = &output.facts.item_effects[&flow_item(&hir, "main")];
        let stdout = Effect::Action(ActionRef {
            tag: CONSOLE_TAG,
            action: CONSOLE_STDOUT_WRITE_ACTION,
        });
        assert!(main.escaping_effects.is_empty());
        assert!(main.requested_actions.effects.contains(&stdout));
        assert!(main.default_actions.effects.contains(&stdout));
        let mut pending = vec![&main.action_trace];
        let mut calls = 0;
        while let Some(trace) = pending.pop() {
            match trace {
                ActionTraceDomain::Event(event) if event.action == stdout => calls += 1,
                ActionTraceDomain::Seq(children) => pending.extend(children),
                ActionTraceDomain::Empty => {}
                other => panic!("unexpected returned record trace: {other:?}"),
            }
        }
        assert_eq!(calls, 2, "{main:?}");
    }
}

#[test]
fn a_nested_composed_pipeline_stage_preserves_each_invoked_handled_action() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow first(value: i32) -> i32 ![] { println("first")?; return value; }
flow second(value: i32) -> i32 ![] { println("second")?; return value; }
flow main() -> i32 {
    let inner = first | second;
    let outer = inner | first;
    return 1 ~> outer;
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
    let main = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(main.escaping_effects.is_empty(), "{main:?}");
    assert!(main.requested_actions.effects.contains(&stdout));
    assert!(main.default_actions.effects.contains(&stdout));
    let mut pending = vec![&main.action_trace];
    let mut calls = 0;
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Event(event) if event.action == stdout => calls += 1,
            ActionTraceDomain::Seq(children) => pending.extend(children),
            ActionTraceDomain::Empty => {}
            other => panic!("unexpected nested composed trace: {other:?}"),
        }
    }
    assert_eq!(calls, 3, "{main:?}");
}

#[test]
fn returned_source_callable_keeps_handled_requests_without_invoking_the_factory_result() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow write() -> unit ![] { println("on invocation")?; return; }
flow choose() -> () -> unit ![] { let selected = write; return selected; }
flow relay() -> () -> unit ![] { return choose(); }
flow main() -> unit { let selected = relay(); selected(); selected(); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for name in ["choose", "relay"] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        assert!(summary.requested_actions.is_empty(), "{summary:?}");
        assert_eq!(summary.action_trace, ActionTraceDomain::Empty);
    }
    let main = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(main.escaping_effects.is_empty(), "{main:?}");
    assert!(main.requested_actions.effects.contains(&stdout));
    assert!(main.default_actions.effects.contains(&stdout));
    assert!(
        matches!(&main.action_trace, ActionTraceDomain::Seq(events)
        if events.len() == 2 && events.iter().all(|event| matches!(event,
            ActionTraceDomain::Event(event) if event.action == stdout))),
        "{main:?}"
    );
}

#[test]
fn a_pipeline_returned_source_callable_cannot_hide_its_handled_trace() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
spec Quiet: trace = -Console.stdout_write;
flow write(value: i32) -> i32 ![] { println("not quiet")?; return value; }
flow identity(value: i32) -> i32 ![] { return value; }
flow choose(flag: bool) -> i32 -> i32 ![] {
    if flag { return identity; }
    return write;
}
flow main(flag: bool) -> i32 ~ Quiet { return 1 ~> choose(flag); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
    assert!(
        !output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn composed_callable_captures_local_targets_and_evaluates_its_factory_only_once() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow identity(value: i32) -> i32 ![] { return value; }
flow write(value: i32) -> i32 ![] { println("invoke")?; return value; }
flow choose() -> i32 -> i32 ![] { println("create")?; return write; }
flow main() -> unit {
    let local = identity;
    let composed = local | choose();
    composed(1);
    composed(2);
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
    let main = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(main.escaping_effects.is_empty(), "{main:?}");
    assert!(main.requested_actions.effects.contains(&stdout));
    let mut pending = vec![&main.action_trace];
    let mut calls = 0;
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Event(event) if event.action == stdout => calls += 1,
            ActionTraceDomain::Seq(children) | ActionTraceDomain::Choice(children) => {
                pending.extend(children)
            }
            ActionTraceDomain::Empty => {}
            other => panic!("unexpected composed trace: {other:?}"),
        }
    }
    assert_eq!(calls, 3, "{main:?}");
}

#[test]
fn dynamic_callable_return_sources_survive_an_escaping_error_path() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow forward(f: (string) -> unit ![Error<IOError>]) -> (string) -> unit ![Error<IOError>] { return f; }
flow relay(fail: bool, f: (string) -> unit ![Error<IOError>]) -> ((string) -> unit ![Error<IOError>]) ![Error<i32>] {
    if fail { perform Error<i32>.raise(7); }
    let indirect = forward;
    return indirect(f);
}
flow main() -> unit ![Error<i32>] {
    let action = relay(false, println);
    action("first")?;
    action("second")?;
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
    let main = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert_eq!(main.escaping_effects.effects.iter().count(), 1);
    assert!(main.escaping_effects.effects.iter().any(|effect| matches!(effect, Effect::Error(ty)
        if matches!(types.store.get(*ty), Some(etas_types::Type::Primitive(etas_types::PrimitiveType::I32))))));
    assert!(main.requested_actions.effects.contains(&stdout));
    assert!(main.default_actions.effects.contains(&stdout));
    let mut pending = vec![&main.action_trace];
    let mut calls = 0;
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Event(event) if event.action == stdout => calls += 1,
            ActionTraceDomain::Seq(children) | ActionTraceDomain::Choice(children) => {
                pending.extend(children)
            }
            ActionTraceDomain::Repeat(child) => pending.push(child),
            ActionTraceDomain::ParameterCall { .. } => panic!("unsubstituted callback trace"),
            _ => {}
        }
    }
    assert_eq!(calls, 2, "{:?}", main.action_trace);
}

#[test]
fn returned_standard_callable_preserves_handled_requests_and_trace_multiplicity() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow factory() -> (string) -> unit ![Error<IOError>] { let alias = println; return alias; }
flow relay(f: (string) -> unit ![Error<IOError>]) -> (string) -> unit ![Error<IOError>] { return f; }
flow main() -> unit {
    let action = relay(factory());
    action("first")?;
    action("second")?;
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
    for name in ["factory", "relay"] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        assert!(summary.requested_actions.is_empty(), "{summary:?}");
        assert_eq!(summary.action_trace, ActionTraceDomain::Empty);
    }
    let main = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(main.escaping_effects.is_empty(), "{main:?}");
    assert!(main.requested_actions.effects.contains(&stdout));
    assert!(main.default_actions.effects.contains(&stdout));
    assert!(
        matches!(&main.action_trace, ActionTraceDomain::Seq(events)
        if events.len() == 2 && events.iter().all(|event| matches!(event,
            ActionTraceDomain::Event(event) if event.action == stdout))),
        "{main:?}"
    );
    assert_eq!(output.facts.std_callable_values.len(), 1);
}

#[test]
fn trace_spec_rejects_handled_actions_from_a_returned_standard_callable() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
spec Quiet: trace = -Console.stdout_write;
flow factory() -> (string) -> unit ![Error<IOError>] { return println; }
flow main() -> unit ~ Quiet { let action = factory(); action("not quiet")?; return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn a_returned_unknown_callable_is_not_replaced_by_its_empty_escaping_row() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow forward(f: () -> unit ![]) -> () -> unit ![] { return f; }
flow main(unknown: { action: () -> unit ![] }) -> unit {
    let action = forward(unknown.action);
    action();
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn returning_a_callable_parameter_preserves_its_source_without_invoking_it() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow forward(f: () -> unit ![]) -> () -> unit ![] { let alias = f; return alias; }
flow relay(g: () -> unit ![]) -> () -> unit ![] { return forward(g); }
flow main() -> unit {
    let action = relay(() => { println("only on invocation")?; return; });
    action();
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
    for name in ["forward", "relay"] {
        let item = flow_item(&hir, name);
        let etas_hir::HirItem::Flow(flow) = &hir.items[item] else {
            panic!("flow expected");
        };
        let parameter = flow.params[0];
        let summary = &output.facts.item_effects[&item];
        assert!(summary.requested_actions.is_empty(), "{summary:?}");
        assert!(matches!(&summary.action_trace, ActionTraceDomain::Empty));
        assert!(summary.exits.as_ref().unwrap().exits.iter().all(|exit| matches!(&exit.outcome,
            etas_effects::CallableOutcome::Returned { observation: etas_effects::ReturnObservation::CallableSources(sources), .. }
            if sources.contains(&etas_effects::CallableReturnSource::Parameter(parameter)))));
    }
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(summary.escaping_effects.is_empty());
    assert!(summary.requested_actions.effects.contains(&stdout));
    assert!(summary.default_actions.effects.contains(&stdout));
    assert!(
        matches!(&summary.action_trace, ActionTraceDomain::Event(event) if event.action == stdout),
        "{summary:?}"
    );
}
