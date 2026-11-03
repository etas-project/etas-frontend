use etas_frontend::{EntryPolicy, Frontend, ProjectInput, SourceInput};

#[test]
fn bounded_file_read_keeps_one_read_request_and_its_independent_limit_error() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module main;
import std.fs.{Region, WorkspacePath, IOError, ReadLimitError, read_bytes_limited};
public flow read<R ~ Region>(path: WorkspacePath<R>) -> bytes ![Error<IOError>, Error<ReadLimitError>] {
    return read_bytes_limited(path, 2048);
}

"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let item = checked
        .items
        .iter()
        .find_map(|(item, declaration)| {
            declaration
                .symbol()
                .and_then(|symbol| checked.symbols.get(symbol))
                .filter(|symbol| symbol.name == "read")
                .map(|_| item)
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&item];
    let Some(etas_effects::Effect::AppliedAction(action)) =
        summary.requested_actions.effects.iter().next()
    else {
        panic!("expected a region-indexed read action: {summary:?}");
    };
    assert!(
        matches!(action.args.as_slice(), [etas_types::EffectArgRef::Type(_)]),
        "region selector must not become a wildcard: {action:?}"
    );
    assert_eq!(
        summary.requested_actions.effects.iter().count(),
        1,
        "{summary:?}"
    );
    assert!(
        matches!(
            summary.action_trace,
            etas_effects::ActionTraceDomain::Event(_)
        ),
        "bounded file read is one action, not a repeated stream reducer: {summary:?}"
    );
    assert_eq!(
        summary.escaping_effects.effects.iter().count(),
        2,
        "{summary:?}"
    );
}

#[test]
fn bounded_file_read_keeps_limit_failure_after_fs_read_is_handled() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module main;
import std.fs.{Region, WorkspacePath, ReadLimitError, read_bytes_limited};
import std.effects.Fs;
public flow read<R ~ Region>(path: WorkspacePath<R>, contents: bytes) -> bytes ![Error<ReadLimitError>] {
    return read_bytes_limited(path, 2048) with { Fs.read<R>(_) => { resume contents; } };
}
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let item = checked
        .items
        .iter()
        .find_map(|(item, declaration)| {
            declaration
                .symbol()
                .and_then(|symbol| checked.symbols.get(symbol))
                .filter(|symbol| symbol.name == "read")
                .map(|_| item)
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&item];
    assert_eq!(
        summary
            .escaping_effects
            .effects
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec![etas_effects::Effect::Error(
            checked.types.known_std_types.file_read_limit_error.unwrap()
        )],
        "{summary:?}"
    );
}

#[test]
fn bounded_file_read_requires_usize_limit_and_declared_error_effect() {
    for (parameters, limit, effects) in [
        ("", "-1", "Error<IOError>, Error<ReadLimitError>"),
        (
            ", maximum: i32",
            "maximum",
            "Error<IOError>, Error<ReadLimitError>",
        ),
        ("", "2048", "Error<IOError>"),
    ] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
            r#"
module main;
import std.fs.{{Region, WorkspacePath, IOError, ReadLimitError, read_bytes_limited}};
public flow read<R ~ Region>(path: WorkspacePath<R>{parameters}) -> bytes ![{effects}] {{
    return read_bytes_limited(path, {limit});
}}
"#
        )));
        input.options.entry_policy = EntryPolicy::Optional;
        let output = Frontend.check_project(input);
        assert!(
            !output.diagnostics.is_empty(),
            "invalid bounded read was accepted"
        );
        assert!(output.checked.is_none());
    }
}
