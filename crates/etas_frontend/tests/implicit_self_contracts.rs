use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn behavioral_spec_method_bounds_cover_escaping_effects_not_internal_requests() {
    for (bound, body, accepted) in [
        ("![]", "perform Console.stdout_write(\"outside\");", false),
        (
            "![Console.stdout_write]",
            "perform Console.stdout_write(\"inside\");",
            true,
        ),
        ("![]", "println(\"handled\")?;", true),
    ] {
        let source = format!(
            r#"
module app.main;
import std.io.println;
spec Reporter {{ flow report(self) -> unit {bound}; }}
type Packet = {{ id: i32 }};
impl Packet ~ Reporter {{ flow report(self) -> unit {{ {body} return; }} }}
flow main() -> unit {{ return; }}
"#
        );
        let output =
            Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
        assert_eq!(
            output.checked.is_some(),
            accepted,
            "{bound} / {body}: {:?}",
            output.diagnostics
        );
        if !accepted {
            assert!(
                output.diagnostics.iter().any(|diagnostic| diagnostic.code
                    == etas_core::DiagnosticCode::Effect(
                        etas_core::EffectDiagnosticCode::EffectOutsideDeclaredRow
                    )),
                "{:?}",
                output.diagnostics
            );
        }
    }
}
