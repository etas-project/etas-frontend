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

#[test]
fn generic_callable_reference_records_its_solved_type_binding() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        r#"
module app;
type UserId = string;
flow identity<T>(value: T) -> T { return value; }
flow apply(callback: (UserId) -> UserId ![], value: UserId) -> UserId {
    return callback(value);
}
flow main() -> UserId { return apply(identity, UserId("checked")); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = etas_types::check_program(&hir);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let reference = hir
        .exprs
        .iter()
        .find_map(|(expr, node)| match node {
            etas_hir::HirExpr::Path(path) if path.segments.last()?.name == "identity" => Some(expr),
            _ => None,
        })
        .unwrap();
    let fact = &output.facts.generic_instantiations[&reference];
    assert_eq!(fact.type_bindings.len(), 1);
    assert_eq!(fact.type_bindings[0].0, "T");
    assert!(matches!(
        output.store.get(fact.type_bindings[0].1),
        Some(Type::Nominal(_))
    ));
}

#[test]
fn generic_callable_reference_enforces_spec_bounds_after_inference() {
    for (subject, succeeds) in [("AllowedId", true), ("OtherId", false)] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(17),
            None,
            format!(
                r#"
module app;
spec Allowed;
type AllowedId = string;
type OtherId = string;
impl AllowedId ~ Allowed;
flow identity<T ~ Allowed>(value: T) -> T {{ return value; }}
flow apply(callback: ({subject}) -> {subject} ![], value: {subject}) -> {subject} {{
    return callback(value);
}}
flow main() -> {subject} {{ return apply(identity, {subject}("checked")); }}
"#
            ),
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let output = etas_types::check_program(&etas_hir::lower_program(&parsed.value));
        assert_eq!(
            output.diagnostics.is_empty(),
            succeeds,
            "{subject}: {:?}",
            output.diagnostics
        );
        if !succeeds {
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("spec bound")),
                "{:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn generic_callable_reference_records_type_and_effect_row_bindings() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        r#"
module app;
type UserId = string;
flow flexible<T, effect E>(value: T) -> T ![E] { return value; }
flow main() -> unit {
    let callback: (UserId) -> UserId ![Error<UserId>] = flexible;
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = etas_types::check_program(&hir);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let reference = hir
        .exprs
        .iter()
        .find_map(|(expr, node)| match node {
            etas_hir::HirExpr::Path(path) if path.segments.last()?.name == "flexible" => Some(expr),
            _ => None,
        })
        .unwrap();
    let fact = &output.facts.generic_instantiations[&reference];
    let subject = fact
        .type_bindings
        .iter()
        .find(|(name, _)| name == "T")
        .unwrap()
        .1;
    assert!(matches!(output.store.get(subject), Some(Type::Nominal(_))));
    assert_eq!(fact.effect_row_bindings.len(), 1);
    let (name, row) = &fact.effect_row_bindings[0];
    assert_eq!(name, "E");
    assert_eq!(row.tail, None);
    assert_eq!(row.effects.len(), 1);
    assert_eq!(row.effects[0].name, "Error");
    assert_eq!(
        row.effects[0].args,
        vec![etas_types::EffectArgRef::Type(subject)]
    );
    let Type::Function(callable) = output
        .store
        .get(output.facts.expr_types[&reference])
        .unwrap()
    else {
        panic!("callable reference must have a checked function type")
    };
    assert_eq!(callable.input, [subject]);
    assert_eq!(callable.output, subject);
    assert_eq!(callable.effects.as_ref(), Some(row));
}

#[test]
fn incompatible_callable_reference_rows_do_not_materialize_instantiation_facts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        r#"
module app;
flow flexible<T, effect E>(value: T) -> T ![Error<T>, E] { return value; }
flow main() -> unit {
    let callback: (string) -> string ![Console.stdout_write] = flexible;
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = etas_types::check_program(&hir);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
    let reference = hir
        .exprs
        .iter()
        .find_map(|(expr, node)| match node {
            etas_hir::HirExpr::Path(path) if path.segments.last()?.name == "flexible" => Some(expr),
            _ => None,
        })
        .unwrap();
    assert!(!output.facts.generic_instantiations.contains_key(&reference));
    assert!(
        output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.primary.span.source == etas_core::SourceId(17))
    );
}
