use etas_frontend::{CheckRequest, FrontendSession, ProjectInput, SourceInput};

#[test]
fn memory_wildcard_import_does_not_export_type_owned_variants() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import std.memory.*;
flow main() -> WriteOutcome<string, i32> {
    return Committed("not a module export");
}
"#,
    )));
    let response = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(response.output.checked.is_none());
    assert!(
        response
            .diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Committed")),
        "{:?}",
        response.diagnostics
    );
}

#[test]
fn memory_outcomes_resolve_associated_constructors_through_type_alias_imports() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
import std.memory.ConfirmedOutcome as Confirmation;
import std.memory.ReconcileResult as Lookup;
flow main() -> Lookup<string, i32> {
    let receipt: Confirmation<string, i32> = Confirmation.Committed("confirmed");
    return Lookup.Found(receipt);
}
"#,
    )));
    let response = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(
        response.output.checked.is_some(),
        "{:?}",
        response.diagnostics
    );
}
