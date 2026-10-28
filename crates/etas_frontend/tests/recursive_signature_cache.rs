use etas_frontend::{
    CheckRequest, FrontendSession, ProjectChangeSet, ProjectInput, ProjectRevision, SourceChange,
    SourceInput, SourceVersion,
};
use std::sync::Arc;

#[test]
fn recursive_representation_changes_invalidate_unchanged_cached_bodies() {
    let source = "module main; type Node = { value: i32, next: Option<Node> }; flow read(node: Node) -> i32 { return node.value; } flow main() -> i32 { return 0; }";
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let first = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(
        first.output.diagnostics.is_empty(),
        "{:?}",
        first.output.diagnostics
    );
    let checked = first.output.checked.as_ref().unwrap();
    let read = checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Flow(flow)
                if checked.symbols.get(flow.symbol).unwrap().name == "read" =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap();
    let before = first
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == read)
        .unwrap()
        .clone();
    let warm = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(
        warm.output.checked.is_some(),
        "{:?}",
        warm.output.diagnostics
    );
    let reused = warm
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == read)
        .unwrap();
    assert!(
        Arc::ptr_eq(&before, reused),
        "the regression must exercise the real body cache"
    );

    // Equal-width replacement leaves every function body and source range intact.
    let changed = source.replace("value: i32", "value: u32");
    assert_eq!(source.len(), changed.len());
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: etas_core::SourceId(0),
                    version: SourceVersion(1),
                    text: changed,
                }],
                dependency_overlay_changes: vec![],
                environment_change: None,
                option_changes: vec![],
            },
        )
        .unwrap();
    let result = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(
        result.output.checked.is_none(),
        "stale recursive signature was reused"
    );
    assert!(
        result
            .output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        result.output.diagnostics
    );
    assert_eq!(
        session.snapshot(project).unwrap().revision,
        ProjectRevision(0)
    );
}
