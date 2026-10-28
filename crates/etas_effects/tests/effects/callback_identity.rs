use super::*;

#[test]
fn lambda_capture_keeps_the_callable_source_from_creation_before_reassignment() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.{println, eprintln};
flow main() -> unit {
    var selected: () -> unit ![] = () => { println("captured")?; return; };
    let invoke = () => { selected(); return; };
    selected = () => { eprintln("later")?; return; };
    invoke();
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
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    let stderr = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDERR_WRITE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        !summary.requested_actions.effects.contains(&stderr),
        "{summary:?}"
    );
    assert!(
        matches!(&summary.action_trace, ActionTraceDomain::Event(event) if event.action == stdout),
        "{summary:?}"
    );
}

#[test]
fn a_captured_callback_alias_and_a_shadowing_parameter_keep_distinct_call_targets() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.{println, eprintln};
flow outer(f: () -> unit ![]) -> unit {
    let captured = f;
    let invoke = (f: () -> unit ![]) => { captured(); f(); return; };
    invoke(() => { eprintln("inner")?; return; });
    return;
}
flow main() -> unit {
    outer(() => { println("outer")?; return; });
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
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    let stderr = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDERR_WRITE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.requested_actions.effects.contains(&stderr),
        "{summary:?}"
    );
    let mut counts = [0, 0];
    let mut pending = vec![&summary.action_trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Event(event) => {
                counts[0] += usize::from(event.action == stdout);
                counts[1] += usize::from(event.action == stderr);
            }
            ActionTraceDomain::Seq(parts) => pending.extend(parts),
            ActionTraceDomain::Empty => {}
            other => panic!("unsubstituted captured callback trace: {other:?}"),
        }
    }
    assert_eq!(counts, [1, 1], "{summary:?}");
}

#[test]
fn an_anonymous_callback_parameter_shadows_the_outer_name_without_losing_its_actions() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
flow main(f: () -> unit ![]) -> unit {
    let invoke = (f: () -> unit ![]) => { f(); return; };
    invoke(() => { println("inner")?; return; });
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
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.default_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    let mut calls = 0;
    let mut pending = vec![&summary.action_trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Event(event) => calls += usize::from(event.action == stdout),
            ActionTraceDomain::Seq(parts) => pending.extend(parts),
            ActionTraceDomain::Empty => {}
            other => panic!("unsubstituted or widened callback trace: {other:?}"),
        }
    }
    assert_eq!(calls, 1, "{summary:?}");
}
