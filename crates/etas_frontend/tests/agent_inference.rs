use etas_frontend::{
    CheckRequest, FrontendSession, ProjectChangeSet, ProjectInput, ProjectRevision, SourceChange,
    SourceInput, SourceVersion,
};

#[test]
fn checked_inference_and_handler_selectors_survive_warm_and_incremental_body_reuse() {
    let source = r#"
module app.main;
agent Writer() -> string { return perform infer<string>(Prompt.new()); }
flow main() -> string {
    return Writer() with { Agentic.infer<_, string>(_, _) => resume "first"; };
}

"#;
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let initial = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    let checked = initial
        .output
        .checked
        .as_ref()
        .expect("initial checked program");
    assert_eq!(checked.types.agent_inferences.len(), 1);
    assert_eq!(checked.types.handler_actions.len(), 1);
    let owner = checked
        .types
        .agent_inferences
        .values()
        .next()
        .unwrap()
        .owner;
    let body = initial
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == owner)
        .unwrap();
    assert_eq!(body.facts.agent_inferences.len(), 1);
    assert!(body.facts.known_std_types.opaque_values.is_empty());
    let warm = session.check(project, CheckRequest::incremental()).unwrap();
    let warm_checked = warm.output.checked.as_ref().expect("warm checked program");
    assert_eq!(
        warm_checked.types.agent_inferences,
        checked.types.agent_inferences
    );
    assert_eq!(
        warm_checked.types.handler_actions,
        checked.types.handler_actions
    );
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
    let changed = session.check(project, CheckRequest::incremental()).unwrap();
    let changed_checked = changed
        .output
        .checked
        .as_ref()
        .expect("edited checked program");
    assert_eq!(
        changed_checked.types.agent_inferences,
        checked.types.agent_inferences
    );
    assert_eq!(
        changed_checked.types.handler_actions,
        checked.types.handler_actions
    );
    let reused = changed
        .output
        .type_body_outputs
        .values()
        .find(|candidate| candidate.owner == owner)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(body, reused),
        "unchanged agent body must retain its checked inference fact without rerunning the checker"
    );
    let summary = &changed_checked.effects.item_effects[&changed_checked.entry.unwrap()];
    assert!(summary.escaping_effects.effects.is_empty());
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert!(matches!(
        &summary.support,
        etas_effects::InterpreterSupport::RequiresInterpreterOrchestration(requirements)
            if requirements.host.kinds.is_empty()
                && requirements.features.kinds.contains(&etas_effects::InterpreterFeatureKind::EffectHandler)
    ));
}
