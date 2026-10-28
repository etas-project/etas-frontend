use etas_effects::{Effect, InterpreterSupport};
use etas_frontend::{Frontend, ProjectInput, ProjectOutput, SourceInput};

fn handler(body: &str) -> ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        format!(
            r#"
module app.main;
effect Probe {{ action request() -> i32; }}
flow main(flag: bool) -> i32 ![] {{
    return handle {{ perform Probe.request() }} with {{
        Probe.request() => {{ {body} }}
    }};
}}
"#
        ),
    )))
}

fn assert_supported(output: &ProjectOutput) {
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().expect("checked handler project");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(
        !matches!(summary.support, InterpreterSupport::Rejected(_)),
        "{summary:?}"
    );
    assert!(summary.escaping_effects.is_empty(), "{summary:?}");
    assert!(summary.escaping_effects.open.is_none());
    assert_eq!(
        summary.requested_actions.effects.iter().count(),
        1,
        "{summary:?}"
    );
    assert!(
        matches!(
            summary.requested_actions.effects.iter().next(),
            Some(Effect::Action(_))
        ),
        "{summary:?}"
    );
    let application = checked.effects.handle_applications.values().next().unwrap();
    assert!(application.remaining.is_empty(), "{application:?}");
    assert!(!application.handled.is_empty(), "{application:?}");
}

#[test]
fn finish_alternative_before_tail_resume_keeps_supported_effect_facts() {
    assert_supported(&handler("if flag { finish 7; } resume 11;"));
}

#[test]
fn abort_alternative_before_tail_resume_keeps_supported_effect_facts() {
    assert_supported(&handler("if flag { abort(\"stopped\"); } resume 11;"));
}

#[test]
fn resume_only_and_finish_only_handler_controls_remain_supported() {
    assert_supported(&handler("resume 11;"));
    assert_supported(&handler("finish 7;"));
}

#[test]
fn malformed_handler_completion_remains_rejected() {
    for body in [
        "if flag { finish 7; }",
        "resume 11; resume 12;",
        "return 7;",
    ] {
        let output = handler(body);
        assert!(output.checked.is_none(), "{body}: {:?}", output.effects);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.severity == etas_core::Severity::Error),
            "{body}: {:?}",
            output.diagnostics
        );
    }
}
