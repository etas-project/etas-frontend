use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(72), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_types::check_program(&etas_hir::lower_program(&parsed.value))
}

#[test]
fn explicit_type_and_effect_arguments_supply_lambda_hints_without_self_bound_rows() {
    let output = check(
        r#"
flow apply<T, effect E>(callback: (T) -> T ![E], value: T) -> T ![E] { return callback(value); }
flow main() -> i32 ![] { return apply<i32, ![]>((value: i32) => { return value; }, 42); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let instantiation = output
        .facts
        .generic_instantiations
        .values()
        .find(|fact| !fact.effect_row_bindings.is_empty())
        .unwrap();
    assert_eq!(
        instantiation.effect_row_bindings,
        vec![(
            "E".into(),
            etas_types::EffectRowRef {
                effects: vec![],
                tail: None
            }
        )]
    );
    assert_eq!(
        output.store.get(instantiation.type_bindings[0].1),
        Some(&etas_types::Type::Primitive(etas_types::PrimitiveType::I32))
    );
}

#[test]
fn recursive_generic_hints_do_not_capture_the_callers_identically_named_parameter() {
    let output = check("flow recur<T>(value: T) -> unit ![] { recur<Array<T>>([value]); }");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.facts.generic_instantiations.values().any(|fact| {
        fact.type_bindings
            .iter()
            .any(|(_, ty)| matches!(output.store.get(*ty), Some(etas_types::Type::Array(_))))
    }));
}

#[test]
fn effect_arguments_do_not_rebind_the_callers_same_named_open_row() {
    let output = check(
        r#"
flow factory<effect E>() -> (() -> unit ![E]) ![] { return () => { return; }; }
flow outer<effect E>() -> (() -> unit ![Console.stdout_write, E]) ![] {
    return factory<![Console.stdout_write, E]>();
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.facts.generic_instantiations.values().any(|fact| {
        fact.effect_row_bindings.iter().any(|(name, row)| {
            name == "E"
                && row.tail.as_deref() == Some("E")
                && row
                    .effects
                    .iter()
                    .any(|effect| effect.name.ends_with("Console.stdout_write"))
        })
    }));
}

#[test]
fn top_level_annotations_reject_incompatible_initializers() {
    for source in [
        "let value: i32 = \"not an integer\";",
        "type UserId = string; let value: UserId = \"not a constructor\";",
        "flow text(value: string) -> string ![] { return value; } let callback: (i32) -> i32 ![] = text;",
    ] {
        let output = check(source);
        assert!(
            output.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic.code,
                DiagnosticCode::Type(
                    TypeDiagnosticCode::Mismatch | TypeDiagnosticCode::TypeMismatch
                )
            )),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn a_known_callable_result_row_is_not_replaced_by_its_contextual_hint() {
    let output = check(
        r#"
flow source() -> (() -> unit ![Console.stdout_write]) ![] { return () => { perform Console.stdout_write("source"); return; }; }
flow wrap<effect E>(callback: () -> unit ![E]) -> (() -> unit ![E]) ![] { return () => { callback(); return; }; }
flow main() -> (() -> unit ![Console.stdout_write]) ![] { return wrap(source()); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output
        .facts
        .generic_instantiations
        .values()
        .find(|fact| {
            !fact.effect_row_bindings.is_empty() || !fact.deferred_effect_row_obligations.is_empty()
        })
        .unwrap();
    assert!(fact.deferred_effect_row_obligations.is_empty(), "{fact:?}");
    assert_eq!(fact.effect_row_bindings.len(), 1);
    let (name, row) = &fact.effect_row_bindings[0];
    assert_eq!(name, "E");
    assert_eq!(row.tail, None);
    assert_eq!(
        row.effects
            .iter()
            .map(|effect| effect.name.as_str())
            .collect::<Vec<_>>(),
        ["Console.stdout_write"]
    );
}

#[test]
fn top_level_callable_annotations_solve_each_generic_reference_independently() {
    let source = r#"
flow identity<T>(value: T) -> T ![] { return value; }
let number: (i32) -> i32 ![] = identity;
let text: (string) -> string ![] = identity;
"#;
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(72), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = etas_types::check_program(&hir);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut count = 0;
    for (_, item) in hir.items.iter() {
        let etas_hir::HirItem::TopLevelLet(binding) = item else {
            continue;
        };
        let expected = match hir.symbols.get(binding.symbol).unwrap().name.as_str() {
            "number" => etas_types::PrimitiveType::I32,
            "text" => etas_types::PrimitiveType::String,
            other => panic!("unexpected binding {other}"),
        };
        let fact = &output.facts.generic_instantiations[&binding.value];
        assert_eq!(fact.type_bindings.len(), 1);
        assert_eq!(
            output.store.get(fact.type_bindings[0].1),
            Some(&etas_types::Type::Primitive(expected))
        );
        let etas_types::Type::Function(signature) = output
            .store
            .get(output.facts.expr_types[&binding.value])
            .unwrap()
        else {
            panic!("checked function reference");
        };
        assert_eq!(signature.input, vec![fact.type_bindings[0].1]);
        assert_eq!(signature.output, fact.type_bindings[0].1);
        count += 1;
    }
    assert_eq!(count, 2);
}

#[test]
fn callback_calls_preserve_the_enclosing_rigid_type_parameter() {
    let output = check(
        r#"
flow apply<T, effect E>(value: T, callback: (T) -> T ![E]) -> T ![E] {
    return callback(value);
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        output
            .facts
            .expr_types
            .values()
            .all(|ty| !matches!(output.store.get(*ty), Some(etas_types::Type::Var(_))))
    );
}

#[test]
fn a_concrete_callback_result_cannot_rebind_a_rigid_return_parameter() {
    let output = check(
        r#"
flow bad<T>(callback: () -> string) -> T {
    return callback();
}
"#,
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn explicit_generic_arguments_bind_a_nested_result_before_match_payload_use() {
    let output = check(
        r#"
type Token<T> = { value: T };
flow wrap<T>(value: T) -> Result<Token<T>, string> { return Ok(Token<T> { value }); }
flow main() -> i32 {
    return match wrap<i32>(42) { Ok(token) => token.value, Err(_) => 0, };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        output
            .facts
            .expr_types
            .values()
            .all(|ty| !matches!(output.store.get(*ty), Some(etas_types::Type::Var(_))))
    );
}
