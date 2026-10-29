use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(completion: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        format!(
            r#"
module app.main;
effect Probe {{ action ask() -> bool; }}
type Value = {{ number: i32 }};
flow controller() -> Result<Value, string> ![Probe] {{
    let selected = perform Probe.ask();
    if selected {{ Ok(Value {{ number = 7 }}) }} else {{ Err("controller") }}
}}
flow main(args: Array<string>) -> i32 {{
    let value = controller() with {{ Probe.ask() => {{ {completion} }} }};
    return match value {{ Ok(value) => value.number, Err(_) => 0 }};
}}
"#
        ),
    )))
}

#[test]
fn inline_finish_constructor_uses_the_concrete_body_answer_type() {
    for completion in [
        "finish Err(\"sentinel\");",
        "finish Ok(Value { number = 11 });",
    ] {
        let output = check(completion);
        assert!(
            output.diagnostics.is_empty(),
            "{completion}: {:?}",
            output.diagnostics
        );
        assert!(output.checked.is_some());
    }
}

#[test]
fn inline_finish_answer_context_does_not_relax_payload_or_nominal_types() {
    for completion in [
        "finish Err(42);",
        "finish Ok(42);",
        "finish Ok({ number = 11 });",
    ] {
        let output = check(completion);
        assert!(output.checked.is_none(), "accepted {completion}");
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
            "{completion}: {:?}",
            output.diagnostics
        );
    }
}
