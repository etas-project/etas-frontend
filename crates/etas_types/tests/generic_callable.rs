use etas_types::{PrimitiveType, Type};

#[test]
fn declared_generic_parameters_are_instantiated_independently_of_their_spelling() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        r#"
module app.main;
type Box<Item> = { value: Item };
flow identity<Content>(value: Box<Content>) -> Box<Content> { return value; }
flow main() -> string {
    let copied = identity(Box<string> { value = "checked" });
    return copied.value;
}

"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let checked = etas_types::check_program(&hir);
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let binding = checked
        .facts
        .generic_instantiations
        .values()
        .flat_map(|fact| &fact.type_bindings)
        .find(|(name, _)| name == "Content")
        .unwrap();
    assert_eq!(
        checked.store.get(binding.1),
        Some(&Type::Primitive(PrimitiveType::String))
    );
}

#[test]
fn generic_record_representation_requires_an_explicit_constructor() {
    for expression in [r#"{ value = "checked" }"#, "Box<string> { value = true }"] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(17),
            None,
            format!(
                "type Box<Item> = {{ value: Item }}; flow main() -> Box<string> {{ return {expression}; }}"
            ),
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let checked = etas_types::check_program(&etas_hir::lower_program(&parsed.value));
        assert!(
            !checked.diagnostics.is_empty(),
            "invalid generic record was accepted: {expression}"
        );
    }
}
