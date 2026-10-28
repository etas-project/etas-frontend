use etas_frontend::{
    CheckRequest, FrontendSession, ProjectChangeSet, ProjectInput, ProjectRevision, SourceChange,
    SourceInput, SourceVersion,
};
use std::sync::Arc;

#[test]
fn editing_one_agent_method_reuses_the_other_checked_body() {
    let source = r#"
module app.main;
agent Worker;
impl agent Worker {
    flow first() -> string { return "first"; }
    flow second() -> i32 { return 42; }
}
flow main() -> string { return Worker.first(); }
"#;
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let initial = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(
        initial.output.diagnostics.is_empty(),
        "{:?}",
        initial.output.diagnostics
    );
    let checked = initial.output.checked.as_ref().unwrap();
    let first = checked
        .types
        .agent_methods
        .values()
        .find(|method| method.name == "first")
        .unwrap()
        .body;
    let second = checked
        .types
        .agent_methods
        .values()
        .find(|method| method.name == "second")
        .unwrap()
        .body;
    let first_body = initial
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == first)
        .unwrap();
    let second_body = initial
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == second)
        .unwrap();
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: etas_core::SourceId(0),
                    version: SourceVersion(1),
                    text: source.replace("\"first\"", "\"later\""),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .unwrap();
    let edited = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(
        edited.output.diagnostics.is_empty(),
        "{:?}",
        edited.output.diagnostics
    );
    let changed = edited
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == first)
        .unwrap();
    let reused = edited
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == second)
        .unwrap();
    assert!(
        !Arc::ptr_eq(first_body, changed),
        "edited method must be checked again"
    );
    assert!(
        Arc::ptr_eq(second_body, reused),
        "unchanged method must reuse its body facts"
    );
    assert_eq!(
        edited
            .output
            .checked
            .as_ref()
            .unwrap()
            .types
            .agent_methods
            .len(),
        2
    );
}
