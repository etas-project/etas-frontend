use etas_core::{DiagnosticCode, EffectDiagnosticCode};
use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)))
}

#[test]
fn a_stream_reducer_has_repeated_requests_and_an_independent_error_row() {
    let output = check(
        r#"
module app;
import std.stream.{ByteStream, ByteLimit, Timeout, StreamError, read_until_limit};
import std.option.None;
type Buffer = string;
impl Buffer ~ ByteStream;
flow main(stream: Buffer) -> bytes ![Error<StreamError>] {
    return read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>());
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(
        summary.requested_actions.effects.iter().count(),
        1,
        "{summary:?}"
    );
    assert_eq!(
        summary.escaping_effects.effects.iter().count(),
        1,
        "{summary:?}"
    );
    assert!(
        !matches!(
            summary.action_trace,
            etas_effects::ActionTraceDomain::Empty | etas_effects::ActionTraceDomain::Event(_)
        ),
        "{summary:?}"
    );
    assert_eq!(
        summary.action_trace.action_set(),
        summary.requested_actions.effects
    );

    let output = check(
        r#"
module app;
import std.stream.{ByteStream, ByteLimit, Timeout, StreamRead, read_until_limit};
import std.effects.Stream;
import std.option.None;
type Buffer = string;
impl Buffer ~ ByteStream;
flow main(stream: Buffer) -> bytes ![] {
    return handle { read_until_limit(stream, ByteLimit { bytes = 4 }, None<Timeout>()) } with {
        Stream.read<Buffer>(_) => { resume StreamRead.Eof; }
    };
}
"#,
    );
    assert!(
        output.checked.is_none(),
        "handling read must not erase the reducer's limit/deadline errors"
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::EffectOutsideDeclaredRow)),
        "{:?}",
        output.diagnostics
    );
}
