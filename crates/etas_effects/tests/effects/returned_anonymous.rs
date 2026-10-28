use super::*;
use etas_effects::CONSOLE_STDERR_WRITE_ACTION;

#[test]
fn recursive_callable_wrappers_preserve_captured_action_behavior() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow wrap(callback: () -> unit ![]) -> (() -> unit ![]) {
    return () => { callback(); return; };
}
flow chain(depth: i32) -> (() -> unit ![]) {
    if depth == 0 { return () => { println("recursive capture")?; return; }; }
    return wrap(chain(depth - 1));
}
flow main(depth: i32) -> unit { chain(depth)(); return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(summary.escaping_effects.is_empty());
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.default_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.action_trace.action_set().contains(&stdout),
        "{summary:?}"
    );
}

#[test]
fn mutually_recursive_generic_wrappers_preserve_nonempty_rows_and_trace_denials() {
    let source = r#"
module app;
flow wrap<effect E>(callback: () -> unit ![E]) -> (() -> unit ![E]) {
    return () => { callback(); return; };
}
flow first(depth: i32) -> (() -> unit ![Console.stdout_write]) {
    if depth == 0 { return () => { perform Console.stdout_write("recursive"); return; }; }
    return wrap(second(depth - 1));
}
flow second(depth: i32) -> (() -> unit ![Console.stdout_write]) {
    if depth == 0 { return () => { perform Console.stdout_write("recursive"); return; }; }
    return wrap(first(depth - 1));
}
spec NoStdout: trace = -Console.stdout_write;
flow main(depth: i32) -> unit ![Console.stdout_write] ~ NoStdout { first(depth)(); return; }
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
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
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.escaping_effects.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.action_trace.action_set().contains(&stdout),
        "{summary:?}"
    );
}

#[test]
fn a_recursive_factory_cannot_hide_an_unavailable_callback_branch() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
effect Factory { action callback() -> (() -> unit ![]); }
flow wrap(callback: () -> unit ![]) -> (() -> unit ![]) { return () => { callback(); return; }; }
flow chain(depth: i32, unknown: bool) -> (() -> unit ![]) ![Factory.callback] {
    if depth == 0 {
        if unknown { return perform Factory.callback(); }
        return () => { return; };
    }
    return wrap(chain(depth - 1, unknown));
}
flow main(depth: i32, unknown: bool) -> unit ![Factory.callback] { chain(depth, unknown)(); return; }
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
fn if_and_match_expression_results_keep_anonymous_callable_sources() {
    for expression in [
        r#"if flag { () => { println("first")?; return; } } else { () => { eprintln("second")?; return; } }"#,
        r#"match flag { true => () => { println("first")?; return; }, false => () => { eprintln("second")?; return; }, }"#,
    ] {
        let source = format!(
            r#"
module app;
import std.io.{{println, eprintln}};
flow choose(flag: bool) -> (() -> unit ![]) {{ return {expression}; }}
flow main(flag: bool) -> unit {{ let selected = choose(flag); selected(); selected(); return; }}
"#
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let mut types = etas_types::check_program(&hir);
        add_std_symbol_type_facts(&hir, &mut types);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let output = check_program(&hir, &types);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let factory = &output.facts.item_effects[&flow_item(&hir, "choose")];
        assert!(factory.requested_actions.is_empty(), "{factory:?}");
        let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
        let mut calls = std::collections::BTreeMap::new();
        let mut pending = vec![&summary.action_trace];
        while let Some(trace) = pending.pop() {
            match trace {
                ActionTraceDomain::Seq(children) | ActionTraceDomain::Choice(children) => {
                    pending.extend(children)
                }
                ActionTraceDomain::Event(event) => {
                    *calls.entry(event.action.clone()).or_insert(0) += 1
                }
                ActionTraceDomain::Empty => {}
                other => panic!("unresolved returned expression trace: {other:?}"),
            }
        }
        for action in [CONSOLE_STDOUT_WRITE_ACTION, CONSOLE_STDERR_WRITE_ACTION] {
            let effect = Effect::Action(ActionRef {
                tag: CONSOLE_TAG,
                action,
            });
            assert!(
                summary.requested_actions.effects.contains(&effect),
                "{summary:?}"
            );
            assert!(
                summary.default_actions.effects.contains(&effect),
                "{summary:?}"
            );
            assert_eq!(calls.get(&effect), Some(&2), "{summary:?}");
        }
    }
}

#[test]
fn an_anonymous_return_in_an_if_branch_cannot_be_hidden_from_a_trace_spec() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.{println, eprintln};
spec NoStdout: trace = -Console.stdout_write;
flow choose(flag: bool) -> (() -> unit ![]) {
    if flag { let selected = () => { println("denied")?; return; }; return selected; }
    return () => { eprintln("allowed")?; return; };
}
flow main(flag: bool) -> unit ~ NoStdout { choose(flag)(); return; }
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
fn anonymous_return_sources_keep_all_branches_and_each_invocation_trace() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.{println, eprintln};
flow choose(flag: bool) -> (() -> unit ![]) {
    if flag {
        let selected = () => { println("first")?; return; };
        return selected;
    }
    return () => { eprintln("second")?; return; };
}
flow main(flag: bool) -> unit {
    let selected = choose(flag);
    selected();
    selected();
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
    let factory = &output.facts.item_effects[&flow_item(&hir, "choose")];
    assert!(factory.requested_actions.is_empty());
    assert_eq!(factory.action_trace, ActionTraceDomain::Empty);
    let main = &output.facts.item_effects[&flow_item(&hir, "main")];
    assert!(main.escaping_effects.is_empty());
    let mut counts = std::collections::BTreeMap::new();
    let mut pending = vec![&main.action_trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Empty => {}
            ActionTraceDomain::Seq(children) | ActionTraceDomain::Choice(children) => {
                pending.extend(children)
            }
            ActionTraceDomain::Event(event) => {
                *counts.entry(event.action.clone()).or_insert(0) += 1
            }
            other => panic!("unresolved anonymous return trace: {other:?}"),
        }
    }
    for action in [CONSOLE_STDOUT_WRITE_ACTION, CONSOLE_STDERR_WRITE_ACTION] {
        let effect = Effect::Action(ActionRef {
            tag: CONSOLE_TAG,
            action,
        });
        assert!(main.requested_actions.effects.contains(&effect), "{main:?}");
        assert!(main.default_actions.effects.contains(&effect), "{main:?}");
        assert_eq!(counts.get(&effect), Some(&2), "{main:?}");
    }
}
