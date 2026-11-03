use etas_syntax::{SourceFile, SourceId, parse_program};

#[test]
fn behavioral_specs_and_nominal_impls_accept_implicit_self_receivers() {
    let parsed = parse_program(SourceFile::new(
        SourceId(1),
        None,
        r#"
spec SnapshotEncoder { flow encode(self) -> i32; }
type Packet = { number: i32 };
impl Packet ~ SnapshotEncoder { flow encode(self) -> i32 { return self.number; } }
impl Packet { flow number(self) -> i32 { return self.number; } }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
}

#[test]
fn implicit_self_is_not_an_untyped_parameter_escape_hatch() {
    for source in [
        "flow free(self) -> unit {}",
        "spec Bad { flow f(value: i32, self) -> unit; }",
        "spec Bad { flow f(value) -> unit; }",
        "flow free() -> unit { let f = (self) => self; }",
    ] {
        let parsed = parse_program(SourceFile::new(SourceId(1), None, source));
        assert!(!parsed.diagnostics.is_empty(), "accepted {source}");
    }
}

#[test]
fn spec_method_selection_composes_with_postfix_and_binary_expressions() {
    for expression in [
        "packet::Failure.fail(\"failed\")?",
        "packet::Encoder.encode() + 1",
        "packet::Encoder.encode().field",
        "packet::Encoder.encode()[0]",
        "packet::Encoder.encode()::Next.finish()",
        "head :: tail",
    ] {
        let source = format!("flow main() -> unit {{ let result = {expression}; }}");
        let parsed = parse_program(SourceFile::new(SourceId(1), None, source));
        assert!(
            parsed.diagnostics.is_empty(),
            "{expression}: {:?}",
            parsed.diagnostics
        );
    }
}
