use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn renamed_payload_constructor_keeps_payload_coverage_and_redundancy() {
    for complete in [true, false] {
        let second = if complete { "Done(false, _) => 2," } else { "" };
        let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(format!(r#"
module app;
import std.http.codec.{{HttpDecodeStep, Complete as Done, NeedMore as Wait, Malformed as Bad}};
flow classify(value: HttpDecodeStep<bool>) -> i32 {{
    return match value {{ Done(true, _) => 1, {second} Wait => 3, Bad(_) => 4, Done(true, _) => 5 }};
}}
flow main() -> i32 {{ return 0; }}
"#))));
        assert_eq!(
            output.checked.is_some(),
            complete,
            "{:?}",
            output.diagnostics
        );
        assert!(
            output.diagnostics.iter().any(|d| d.code
                == etas_core::DiagnosticCode::Type(
                    etas_core::TypeDiagnosticCode::RedundantMatchArm
                )),
            "{:?}",
            output.diagnostics
        );
        assert_eq!(
            output
                .diagnostics
                .iter()
                .filter(|d| d.code
                    == etas_core::DiagnosticCode::Type(
                        etas_core::TypeDiagnosticCode::NonExhaustiveMatch
                    ))
                .count(),
            usize::from(!complete),
            "{:?}",
            output.diagnostics
        );
    }
}

#[test]
fn renamed_std_constructors_preserve_match_coverage() {
    let input = ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app;
import std.codec.text.{MalformedInput, Strict as S, Replace};
flow classify(value: MalformedInput) -> i32 {
    return match value { S => 1, Replace => 2 };
}
flow main() -> i32 { return classify(S); }
"#,
    ));
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}

#[test]
fn imported_named_enum_constructors_enforce_spec_bounds() {
    for implementation in ["", "impl Value ~ Allowed;"] {
        for expr in [
            "Box.Named { value = Value(42) }",
            "Box.Named<Value> { value = Value(42) }",
        ] {
            let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
                "module app; import data.{{Box, Value}}; flow main() -> Box<Value> {{ return {expr}; }}"
            )));
            let mut dependency = SourceInput::anonymous(format!(
                "module data; public spec Allowed; public type Value = i32; {implementation} public enum Box<T ~ Allowed> {{ Named {{ value: T }} }}"
            ));
            dependency.id = etas_core::SourceId(1);
            input.sources.push(dependency);
            let output = Frontend.check_project(input);
            assert_eq!(
                output.checked.is_some(),
                !implementation.is_empty(),
                "{expr}: {:?}",
                output.diagnostics
            );
            if implementation.is_empty() {
                assert!(
                    output
                        .diagnostics
                        .iter()
                        .any(|d| d.message.contains("does not satisfy spec bound")),
                    "{:?}",
                    output.diagnostics
                );
            }
        }
    }
}

#[test]
fn source_enum_members_resolve_through_imports_and_visibility() {
    for public in [true, false] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
module app;
import data.Tree;
flow main() -> Tree<i32> { return Tree.Leaf(7); }
"#,
        ));
        let mut dependency = SourceInput::anonymous(format!(
            "module data; {} enum Tree<T> {{ Leaf(T), Branch(Tree<T>, Tree<T>), }}",
            if public { "public" } else { "" }
        ));
        dependency.id = etas_core::SourceId(1);
        input.sources.push(dependency);
        let output = Frontend.check_project(input);
        assert_eq!(output.checked.is_some(), public, "{:?}", output.diagnostics);
    }
}

#[test]
fn imported_named_constructors_patterns_and_aliases() {
    for (import, name) in [
        ("import data.Response;", "Response"),
        ("import data.Response as Reply;", "Reply"),
        ("import data.*;", "Response"),
    ] {
        let mut input = ProjectInput::single_source(SourceInput::anonymous(format!(
            r#"
module app;
{import}
flow main() -> string {{
    let value = {name}.Failure {{ code = 503, message = "unavailable" }};
    return match value {{ {name}.Success(text) => text, {name}.Failure {{ code: _, message }} => message, _ => "unreachable" }};
}}
"#
        )));
        let mut dependency = SourceInput::anonymous(
            "module data; public enum Response { Success(string), Failure { code: i32, message: string } }",
        );
        dependency.id = etas_core::SourceId(1);
        input.sources.push(dependency);
        let output = Frontend.check_project(input);
        assert!(
            output.checked.is_some(),
            "{import}: {:?}",
            output.diagnostics
        );
        assert_eq!(
            output.diagnostics.len(),
            1,
            "{import}: {:?}",
            output.diagnostics
        );
        assert_eq!(
            output.diagnostics[0].code,
            etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::RedundantMatchArm)
        );
        assert_eq!(output.diagnostics[0].severity, etas_core::Severity::Warning);
    }
}
