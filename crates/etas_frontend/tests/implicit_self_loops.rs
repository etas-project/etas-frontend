use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn generic_spec_method_evidence_survives_nested_wrappers_and_loop_widening() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import std.io.println;
spec Reporter { flow report(self) -> unit; }
type Packet = { id: i32 };
impl Packet ~ Reporter { flow report(self) -> unit { println("reported")?; return; } }
flow inner<T ~ Reporter>(value: T) -> unit {
    for index in [1, 2] { value::Reporter.report(); }
}
flow outer<U ~ Reporter>(value: U) -> unit { inner(value); }
flow main() -> unit { outer(Packet { id = 0 }); }
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked nested generic loop");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty());
    assert!(!summary.requested_actions.is_empty());
    assert!(
        !summary.action_trace.action_set().is_empty(),
        "widening dropped the method: {summary:?}"
    );
    assert!(
        summary.action_trace.parameter_calls().is_empty(),
        "closed entry retained unresolved evidence: {summary:?}"
    );
}
