use etas_core::{SourceFile, SourceId};

#[test]
fn std_enum_layouts_drive_payload_coverage_and_redundancy() {
    let output = check(
        r#"
import std.http.codec.{HttpCodecError, MalformedMessage};
flow label(error: HttpCodecError) -> string {
    return match error { MalformedMessage => "malformed", _ => "other" };
}
"#,
    );
    assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
    assert_eq!(
        output.diagnostics[0].code,
        etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::RedundantMatchArm)
    );
    assert_eq!(output.diagnostics[0].severity, etas_core::Severity::Warning);
}

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(1), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    etas_types::check_program(&hir)
}

#[test]
fn recursive_tree_and_positional_constructors() {
    let output = check(
        r#"
module app;
enum Tree<T> { Leaf(T), Branch(Tree<T>, Tree<T>), }
flow main() -> Tree<i32> {
    return Tree.Branch(Tree.Leaf(1), Tree.Leaf(2));
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn mutually_recursive_nominal_headers() {
    let output = check(
        r#"
module app;
type Directory = { name: string, entries: Array<Entry> }
enum Entry { File { name: string, content: bytes }, Subdirectory(Directory), }
flow identity(value: Directory) -> Directory { return value; }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn recursive_transparent_alias_is_rejected() {
    let output = check("alias Bad = Array<Bad>; flow f(x: Bad) -> unit { return; }");
    assert!(!output.diagnostics.is_empty());
}

#[test]
fn named_variant_construction_and_pattern_shorthand() {
    let output = check(
        r#"
module app;
enum Response { Success(string), Failure { code: i32, message: string }, }
flow main() -> string {
    let message = "unavailable";
    let response = Response.Failure { code = 503, message };
    return match response {
        Response.Success(text) => text,
        Response.Failure { code: _, message } => message,
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn enum_rejects_invalid_fields_and_payload_style() {
    for expr in [
        "Response.Failure { code = 1 }",
        "Response.Failure { code = 1, message = \"a\", extra = 2 }",
        "Response.Failure { code = 1, code = 2, message = \"a\" }",
        "Response.Failure { code = \"wrong\", message = \"a\" }",
        "Response.Failure(1, \"a\")",
        "Response.Success { value = \"a\" }",
        "Response.Missing { code = 1 }",
    ] {
        let output = check(&format!(
            "enum Response {{ Success(string), Failure {{ code: i32, message: string }}, }} flow main() -> Response {{ return {expr}; }}"
        ));
        assert!(!output.diagnostics.is_empty(), "accepted {expr}");
    }
}

#[test]
fn match_checks_payload_coverage_and_reports_redundant_arms() {
    for body in [
        "return match value { Choice.A(1) => 1, Choice.B => 2 };",
        "match value { Choice.A(1) => { return 1; }, Choice.B => { return 2; } } return 0;",
    ] {
        let output = check(&format!(
            "enum Choice {{ A(i32), B }} flow f(value: Choice) -> i32 {{ {body} }}"
        ));
        assert!(
            output.diagnostics.iter().any(|d| d.code
                == etas_core::DiagnosticCode::Type(
                    etas_core::TypeDiagnosticCode::NonExhaustiveMatch
                )),
            "{:?}",
            output.diagnostics
        );
    }
    let output = check(
        "enum Choice { A(bool), B } flow f(value: Choice) -> i32 { return match value { Choice.A(true) => 1, Choice.A(false) => 2, Choice.B => 3, _ => 4 }; }",
    );
    assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
    assert_eq!(output.diagnostics[0].severity, etas_core::Severity::Warning);
}

#[test]
fn named_patterns_require_exact_fields_and_correct_payload_types() {
    for pattern in [
        "Response.Failure { code: _ }",
        "Response.Failure { code: _, message: _, extra: _ }",
        "Response.Failure { code: _, code: _, message: _ }",
        "Response.Failure { code: true, message: _ }",
        "Response.Failure(_, _)",
    ] {
        let output = check(&format!(
            "enum Response {{ Failure {{ code: i32, message: string }} }} flow f(value: Response) -> unit {{ match value {{ {pattern} => {{ return; }}, _ => {{ return; }} }} }}"
        ));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.severity == etas_core::Severity::Error),
            "accepted {pattern}"
        );
    }
}

#[test]
fn generic_named_payload_and_recursive_alias_boundaries() {
    let output = check(
        "enum Tree<T> { Leaf { value: T }, Branch(Tree<T>, Tree<T>) } alias IntTree = Tree<i32>; flow f() -> IntTree { return Tree.Leaf { value = 1 }; }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for source in [
        "enum Tree<T> { Leaf { value: T } } flow f() -> Tree<i32> { return Tree.Leaf { value = true }; }",
        "enum Tree<T> { Leaf(T) } flow f() -> Tree<i32> { return Tree.Leaf(true); }",
        "alias A = Array<B>; alias B = Option<A>; flow f(x: A) -> unit { return; }",
        "enum E { A, A } flow f() -> unit { return; }",
        "enum E { A { x: i32, x: bool } } flow f() -> unit { return; }",
    ] {
        let output = check(source);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.severity == etas_core::Severity::Error),
            "accepted {source}"
        );
    }
}

#[test]
fn coverage_preserves_payload_correlations_and_terminates_for_recursive_types() {
    let output = check(
        "enum Pair { P(bool, bool) } flow f(x: Pair) -> unit { match x { Pair.P(true, true) => { return; }, Pair.P(false, false) => { return; } } }",
    );
    assert!(output.diagnostics.iter().any(|d| d.code
        == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::NonExhaustiveMatch)));
    let output = check(
        "enum Chain { End, Next(Chain) } flow f(x: Chain) -> unit { match x { Chain.End => { return; }, Chain.Next(Chain.End) => { return; } } }",
    );
    assert!(output.diagnostics.iter().any(|d| d.code
        == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::NonExhaustiveMatch)));
}
