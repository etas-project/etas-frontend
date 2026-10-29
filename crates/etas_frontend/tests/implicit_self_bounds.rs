use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn spec_method_implementation_cannot_strengthen_its_generic_contract() {
    for (declared, implemented, accepted) in [
        ("T", "U ~ Allowed", false),
        ("T ~ Allowed", "U ~ Allowed", true),
        ("T ~ Strict", "U ~ Allowed", true),
    ] {
        let source = format!(
            r#"
module app.main;
spec Allowed;
spec Strict ~ Allowed;
spec Identity {{ flow identity<{declared}>(self, value: T) -> T; }}
type Packet = {{ id: i32 }};
impl Packet ~ Identity {{ flow identity<{implemented}>(self, value: U) -> U {{ return value; }} }}
flow main() -> unit {{ return; }}
"#
        );
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert_eq!(
            output.checked.is_some(),
            accepted,
            "{declared} / {implemented}: {:?}",
            output.diagnostics
        );
        if !accepted {
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("strengthens")),
                "{:?}",
                output.diagnostics
            );
        }
    }
}
