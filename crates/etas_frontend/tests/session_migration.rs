use etas_frontend::{Frontend, SourceInput};

#[test]
fn session_config_uses_selection_and_retention_without_automatic_inference() {
    let output = Frontend.check(SourceInput::anonymous(
        r#"
module app.main;
flow main(ticket: SessionId) -> SessionConfig {
    return SessionConfig { id = ticket, context = SummaryPlusRecent(4), retention = Days(90) };
}
"#,
    ));
    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn obsolete_session_compaction_is_rejected_not_ignored() {
    for source in [
        "flow main(limit: Limit) -> unit { let old = SummarizeWhen(limit); return; }",
        "flow main(old: CompactionPolicy) -> unit { return; }",
        "flow main(c: SessionConfig) -> Conversation { return Conversation.compact(c); }",
        "import std.agent.session.compact; flow main(c: SessionConfig) -> Conversation { return compact(c); }",
        "flow main(id: SessionId) -> SessionConfig { return SessionConfig { id = id, context = LastTurns(4), retention = Days(90), compaction = () }; }",
    ] {
        let output = Frontend.check(SourceInput::anonymous(source));
        assert!(
            output.checked.is_none(),
            "obsolete source was accepted: {source}"
        );
        assert!(
            !output.diagnostics.is_empty(),
            "missing rejection: {source}"
        );
    }
}
