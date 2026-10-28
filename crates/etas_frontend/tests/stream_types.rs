use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)))
}

#[test]
fn stream_enums_have_typed_payloads_and_exhaustive_patterns() {
    let output = check(
        r#"
module app;
import std.stream.{StreamRead, StreamError};
flow result(value: bytes) -> StreamRead { return StreamRead.Data(value); }
flow classify(value: StreamRead) -> i32 {
    return match value { StreamRead.Data(_) => 1, StreamRead.Eof => 0, };
}
flow error(value: StreamError) -> bool {
    return match value {
        StreamError.TimedOut => false, StreamError.Cancelled => false,
        StreamError.Closed => false, StreamError.Interrupted => false,
        StreamError.LimitExceeded => false, StreamError.Host(_) => true,
    };
}
flow main() -> i32 { return classify(StreamRead.Eof); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    for name in ["std.stream.StreamRead", "std.stream.StreamError"] {
        let (ty, _) = checked
            .type_store
            .iter()
            .find(|(_, ty)| matches!(ty, etas_types::Type::Enum(decl) if decl.name == name))
            .unwrap();
        assert!(checked.types.enum_layouts.contains_key(&ty));
    }
}

#[test]
fn stream_enum_payload_and_coverage_errors_are_rejected() {
    for (body, code) in [
        (
            "flow main() -> StreamRead { return StreamRead.Data(42); }",
            TypeDiagnosticCode::TypeMismatch,
        ),
        (
            "flow main(value: StreamRead) -> unit { match value { StreamRead.Eof => {}, } return; }",
            TypeDiagnosticCode::NonExhaustiveMatch,
        ),
        (
            "flow main(value: StreamRead) -> i32 { return match value { StreamRead.Eof => 0, }; }",
            TypeDiagnosticCode::NonExhaustiveMatch,
        ),
    ] {
        let output = check(&format!("module app; import std.stream.StreamRead; {body}"));
        assert!(output.checked.is_none());
        let expected = DiagnosticCode::Type(code);
        assert!(
            output.diagnostics.iter().any(|d| d.code == expected),
            "{:?}",
            output.diagnostics
        );
    }
}

#[test]
fn returning_an_enum_with_an_empty_row_does_not_make_a_flow_a_pure_constructor() {
    let output = check(
        r#"
module app;
import std.stream.StreamRead;
import std.io.println;
flow make(input: bytes) -> StreamRead ![] {
    let printed = println("requested action remains visible")?;
    return StreamRead.Data(input);
}

flow main(input: bytes) -> StreamRead ![] {
    let constructor = make;
    return constructor(input);
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.effects.is_empty());
    assert!(
        summary
            .requested_actions
            .effects
            .contains(&etas_effects::Effect::Action(etas_effects::ActionRef {
                tag: etas_effects::CONSOLE_TAG,
                action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
            })),
        "{summary:?}"
    );
}
