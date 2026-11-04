use etas_types::{Type, check_program, method::MethodTarget};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(1),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    check_program(&hir)
}

#[test]
fn existential_method_lookup_uses_declared_and_inherited_evidence_without_a_concrete_impl() {
    let checked = check(
        r#"
spec Named { flow name(self) -> string ![]; }
spec Printable ~ Named;
flow read(value: ? ~ Printable) -> string ![] { return value.name(); }
flow selected(value: ? ~ Printable) -> string ![] { return value::Named.name(); }
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(checked.facts.checked_method_calls.len(), 2);
    for call in checked.facts.checked_method_calls.values() {
        assert!(matches!(call.target, MethodTarget::BoundSpec { .. }));
        assert!(matches!(
            checked.store.get(call.receiver_type),
            Some(Type::Existential { .. })
        ));
    }
}

#[test]
fn existential_marker_bounds_keep_their_standard_declaration() {
    let checked = check(
        "import std.stream.ByteStream; flow retain(value: ? ~ ByteStream) -> ? ~ ByteStream { return value; }",
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn existential_spec_arguments_flow_into_chained_method_results() {
    let checked = check(
        r#"
spec Named<T> { flow name(self) -> T ![]; }
flow size(value: ? ~ Named<string>) -> usize ![] { return value.name().len(); }
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(checked.facts.checked_method_calls.len(), 2);
}

#[test]
fn a_generic_behavioral_method_cannot_be_hidden_in_an_existential() {
    let checked = check(
        r#"
spec Generic { flow identity<T>(self, value: T) -> T; }
flow retain(value: ? ~ Generic) -> ? ~ Generic { return value; }
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("not object-safe")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn contextual_existential_packing_records_the_concrete_type_and_checked_evidence() {
    let checked = check(
        r#"
spec Named { flow name(self) -> string ![]; }
type Label = string;
impl Label ~ Named { flow name(self) -> string ![] { return "label"; } }
flow read(value: ? ~ Named) -> string ![] { return value.name(); }
flow main() -> string ![] {
    let value: ? ~ Named = Label("example");
    return read(value);
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(checked.facts.existential_packs.len(), 1);
    let pack = checked.facts.existential_packs.values().next().unwrap();
    assert!(matches!(
        checked.store.get(pack.source_type),
        Some(Type::Nominal(_))
    ));
    assert!(matches!(
        checked.store.get(pack.object_type),
        Some(Type::Existential { .. })
    ));
    assert_eq!(pack.witnesses.len(), 1);
    assert!(matches!(
        pack.witnesses[0].target,
        MethodTarget::Source { .. }
    ));
}
