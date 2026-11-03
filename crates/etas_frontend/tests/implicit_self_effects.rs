use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn generic_spec_dispatch_uses_only_the_selected_implementations_request_trace() {
    for (selected, requests) in [("Quiet", false), ("Loud", true)] {
        let source = format!(
            r#"
module app.main;
import std.io.println;
spec Reporter {{ flow report(self) -> unit; }}
type Quiet = {{ id: i32 }};
type Loud = {{ id: i32 }};
impl Quiet ~ Reporter {{ flow report(self) -> unit {{ return; }} }}
impl Loud ~ Reporter {{ flow report(self) -> unit {{ println("reported")?; return; }} }}
flow twice<T ~ Reporter>(value: T) -> unit {{
    value::Reporter.report();
    value::Reporter.report();
}}
flow main() -> unit {{ twice({selected} {{ id = 0 }}); }}
"#
        );
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert!(
            output.diagnostics.is_empty(),
            "{selected}: {:?}",
            output.diagnostics
        );
        let checked = output.checked.expect("checked generic request trace");
        let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
        assert!(summary.escaping_effects.is_empty(), "{summary:?}");
        assert_eq!(
            !summary.requested_actions.is_empty(),
            requests,
            "{selected}: {summary:?}"
        );
        if requests {
            let etas_effects::ActionTraceDomain::Seq(events) = &summary.action_trace else {
                panic!("generic invocation multiplicity disappeared: {summary:?}");
            };
            assert_eq!(events.len(), 2);
            assert!(
                events
                    .iter()
                    .all(|event| matches!(event, etas_effects::ActionTraceDomain::Event(_)))
            );
        } else {
            assert_eq!(summary.action_trace, etas_effects::ActionTraceDomain::Empty);
        }
    }
}
