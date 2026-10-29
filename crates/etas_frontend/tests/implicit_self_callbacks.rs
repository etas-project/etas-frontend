use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn generic_spec_method_infers_callback_effect_row_without_losing_trace_positions() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
spec Runner {
    flow run<effect E>(self, callback: () -> unit ![E]) -> unit ![E];
}
type Packet = { id: i32 };
impl Packet ~ Runner {
    flow run<effect F>(self, callback: () -> unit ![F]) -> unit ![F] {
        callback();
        callback();
    }
}
flow invoke<T ~ Runner>(value: T) -> unit ![Console.stdout_write] {
    value::Runner.run(() => { perform Console.stdout_write("callback"); return; });
}
flow main() -> unit ![Console.stdout_write] { invoke(Packet { id = 7 }); }
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked inferred method callback");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(summary.escaping_effects.effects.iter().count(), 1);
    assert!(summary.escaping_effects.open.is_none());
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!("inferred callback trace lost invocation positions: {summary:?}");
    };
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| matches!(event, etas_effects::ActionTraceDomain::Event(_)))
    );
}

fn paired_callback_source(callbacks: &str) -> String {
    format!(
        r#"
module app.main;
import std.io.println;
spec Runner {{
    flow run<effect E>(self, f: () -> unit ![E], g: () -> unit ![E]) -> unit ![E];
}}
type Packet = {{ id: i32 }};
impl Packet ~ Runner {{
    flow run<effect F>(self, f: () -> unit ![F], g: () -> unit ![F]) -> unit ![F] {{
        f(); g(); f();
    }}
}}
flow invoke<T ~ Runner>(value: T) -> unit ![] {{ value::Runner.run({callbacks}); }}
flow main() -> unit ![] {{ invoke(Packet {{ id = 7 }}); }}
"#
    )
}

#[test]
fn generic_spec_method_unifies_callback_rows_not_their_requested_actions() {
    let source =
        paired_callback_source("() => { println(\"handled\")?; return; }, () => { return; }");
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked equal callback rows");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty());
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!("handled callback trace disappeared: {summary:?}");
    };
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| matches!(event, etas_effects::ActionTraceDomain::Event(_)))
    );
}

#[test]
fn generic_spec_method_rejects_callbacks_with_different_escaping_rows() {
    let source = paired_callback_source(
        "() => { perform Console.stdout_write(\"escaping\"); return; }, () => { return; }",
    );
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Effect(
                    etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                )
                && diagnostic.message.contains("do not agree on the row")
        }),
        "{:?}",
        output.diagnostics
    );
    assert!(output.checked.is_none());
}

#[test]
fn generic_spec_dispatch_keeps_callback_arguments_and_call_positions() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import std.io.println;
spec Runner { flow run(self, callback: () -> unit ![]) -> unit; }
type Packet = { id: i32 };
impl Packet ~ Runner {
    flow run(self, callback: () -> unit ![]) -> unit { callback(); callback(); }
}
flow invoke<T ~ Runner>(value: T, callback: () -> unit ![]) -> unit {
    value::Runner.run(callback);
}
flow main() -> unit {
    invoke(Packet { id = 7 }, () => { println("callback")?; return; });
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked method callback");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty());
    assert!(!summary.requested_actions.is_empty());
    let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
        panic!("callback invocation positions disappeared: {summary:?}");
    };
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| matches!(event, etas_effects::ActionTraceDomain::Event(_)))
    );
}
