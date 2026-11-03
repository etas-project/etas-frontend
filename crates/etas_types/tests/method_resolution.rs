use etas_types::{check_program, method::MethodTarget};

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
fn inherent_spec_and_generic_calls_share_checked_targets() {
    let output = check(include_str!("fixtures/method_resolution/mixed.es"));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.checked_method_calls.len(), 7);
    assert_eq!(
        output
            .facts
            .checked_method_calls
            .values()
            .filter(|fact| matches!(fact.target, MethodTarget::BoundSpec { .. }))
            .count(),
        1
    );
    assert!(
        output
            .facts
            .checked_method_calls
            .values()
            .all(|fact| output.store.get(fact.signature.output).is_some())
    );
}

#[test]
fn ambiguous_default_method_is_rejected_but_explicit_selection_has_a_target() {
    let output = check(include_str!("fixtures/method_resolution/ambiguous.es"));
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("ambiguous")),
        "{:?}",
        output.diagnostics
    );
    assert_eq!(output.facts.checked_method_calls.len(), 1);
}

#[test]
fn delayed_method_results_materialize_generic_record_projections() {
    let output = check(include_str!(
        "fixtures/method_resolution/generic_projection.es"
    ));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.checked_method_calls.len(), 2);
}

#[test]
fn method_result_substitutes_effect_arguments_in_the_callers_namespace() {
    let output = check(include_str!("fixtures/method_resolution/outer_row.es"));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.checked_method_calls.len(), 2);
    for fact in output.facts.checked_method_calls.values() {
        let Some(etas_types::Type::Function(result)) = output.store.get(fact.signature.output)
        else {
            panic!("method result is not a callable");
        };
        let row = result.effects.as_ref().unwrap();
        assert_eq!(row.tail.as_deref(), Some("E"));
        assert_eq!(row.effects.len(), 1);
        assert_eq!(row.effects[0].name, "Console.stdout_write");
        assert!(
            fact.instantiation
                .deferred_effect_row_obligations
                .is_empty()
        );
    }
}

#[test]
fn spec_parameter_labels_survive_bound_and_implementation_selection() {
    let output = check(include_str!("fixtures/method_resolution/named_bound.es"));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.checked_method_calls.len(), 3);
    assert_eq!(
        output
            .facts
            .checked_method_calls
            .values()
            .filter(|fact| matches!(fact.target, MethodTarget::BoundSpec { .. }))
            .count(),
        2
    );
    for fact in output.facts.checked_method_calls.values() {
        assert_eq!(fact.argument_slots, [1]);
    }
}

#[test]
fn long_source_method_chains_are_collected_without_native_recursion() {
    let source = format!(
        "module main; type Counter = {{ value: i32 }}; \
         impl Counter {{ flow copy(self) -> Counter {{ return self; }} }} \
         flow main() -> i32 {{ return Counter {{ value = 4 }}{}.value; }}",
        ".copy()".repeat(256)
    );
    let output = check(&source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.checked_method_calls.len(), 256);
}

#[test]
fn checked_method_abi_substitutes_rows_simultaneously_without_capturing_caller_binders() {
    let output = check(include_str!("fixtures/method_resolution/row_namespace.es"));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let [fact] = output
        .facts
        .checked_method_calls
        .values()
        .collect::<Vec<_>>()[..]
    else {
        panic!("expected one checked method");
    };
    for ty in [fact.signature.params[0], fact.signature.output] {
        let Some(etas_types::Type::Function(flow)) = output.store.get(ty) else {
            panic!("expected a callable type");
        };
        let row = flow.effects.as_ref().unwrap();
        assert_eq!(row.tail.as_deref(), Some("F"));
        assert!(row.effects.is_empty(), "{row:?}");
    }
    let Some(etas_types::Type::Function(console)) = output.store.get(fact.signature.params[1])
    else {
        panic!("expected the console callable parameter");
    };
    let row = console.effects.as_ref().unwrap();
    assert!(row.tail.is_none());
    assert_eq!(row.effects.len(), 1);
    assert_eq!(row.effects[0].name, "Console.stdout_write");
}
