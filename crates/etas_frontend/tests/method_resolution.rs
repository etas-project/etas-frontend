use etas_frontend::{CheckRequest, CheckScope, FrontendSession, ProjectInput, SourceInput};
use etas_types::method::MethodTarget;

#[test]
fn entry_reachability_follows_only_the_selected_method_bodies() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(
        include_str!("fixtures/method_resolution/reachable_chain.es"),
    )));
    let output = session
        .check(
            project,
            CheckRequest::full_project().with_scope(CheckScope::EntryReachable),
        )
        .unwrap()
        .output;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_eq!(checked.types.checked_method_calls.len(), 2);
    for fact in checked.types.checked_method_calls.values() {
        let MethodTarget::Source { callable, body, .. } = fact.target else {
            panic!("expected a direct checked source body");
        };
        assert!(checked.reachability.reachable_items.contains(&callable));
        assert!(checked.reachability.analysis_items.contains(&callable));
        assert_eq!(checked.hir.view().body(body).unwrap().item().id(), callable);
    }
    assert_eq!(checked.reachability.reachable_items.len(), 3);
}
