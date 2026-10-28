use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};
use etas_types::{Type, TypeId};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(0), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_types::check_program(&etas_hir::lower_program(&parsed.value))
}

#[test]
fn std_iterable_element_types_survive_aliases_generic_calls_and_finalization() {
    for name in ["Deque", "Queue", "Stack"] {
        for source in [
            format!(
                "flow first<T>(xs: {name}<T>) -> Option<T> {{ for x in xs limit Iterations(1) {{ return Some(x); }} return None; }} flow main(xs: {name}<string>) -> Option<string> {{ return first(xs); }}"
            ),
            format!(
                "alias Values<T> = {name}<T>; flow main(xs: Values<string>) -> string {{ for x in xs limit Iterations(1) {{ return x; }} return \"\"; }}"
            ),
        ] {
            let output = check(&source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            let expected = format!("std.collections.{name}");
            let (constructor, fact) = output
                .facts
                .known_std_types
                .iterables
                .iter()
                .find(|(ty, _)| etas_types::ty::display_type(&output.store, **ty) == expected)
                .unwrap();
            assert_eq!(fact.arity, 1);
            assert_eq!(
                fact.item,
                etas_types::facts::symbols::StdIterableItemFact::Element { parameter: 0 }
            );
            assert!(output.store.iter().any(|(_, ty)| matches!(ty, Type::Applied { constructor: applied, .. } if TypeId(applied.0) == *constructor)));
        }
    }
}

#[test]
fn std_iterable_checks_item_type_and_does_not_project_unrelated_nominals() {
    for name in ["Deque", "Queue", "Stack"] {
        for source in [
            format!(
                "flow main(xs: {name}<string>) -> i32 {{ for x in xs limit Iterations(1) {{ return x; }} return 0; }}"
            ),
            format!(
                "type {name}<T> = {{ item: T }}; flow main(xs: {name}<i32>) -> unit {{ for x in xs limit Iterations(1) {{ }} return; }}"
            ),
            format!(
                "type Wrapped<T> = {name}<T>; flow main(xs: Wrapped<i32>) -> unit {{ for x in xs limit Iterations(1) {{ }} return; }}"
            ),
        ] {
            let output = check(&source);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn std_entry_iterables_bind_declared_key_and_value_parameters() {
    for (container, parameters) in [("OrderedMap", "K, V"), ("PriorityQueue", "V, K")] {
        let source = format!(
            "alias Entries<K, V> = {container}<{parameters}>; \
             flow visit<K, V>(entries: Entries<K, V>) -> unit {{ \
                 for (key, value) in entries limit Iterations(8) {{ \
                     let expected_key: K = key; let expected_value: V = value; \
                 }} \
                 for entry in entries limit Iterations(8) {{ let pair: (K, V) = entry; }} \
             }}"
        );
        let output = check(&source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn std_ordered_set_iterator_preserves_nominal_element_identity() {
    let output = check(
        "type Key = i32; flow visit(entries: OrderedSet<Key>) -> unit { \
             for key in entries limit Iterations(8) { let expected: Key = key; } \
         }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn std_entry_iterables_reject_wrong_types_arity_and_nominal_wrappers() {
    for source in [
        "flow visit(entries: OrderedMap<string, i32>) -> unit { for (key, value) in entries limit Iterations(8) { let wrong: i32 = key; } }",
        "flow visit(entries: OrderedMap<string, i32>) -> unit { for (key, value) in entries limit Iterations(8) { let wrong: string = value; } }",
        "flow visit(entries: OrderedMap<string, i32>) -> unit { for (a, b, c) in entries limit Iterations(8) {} }",
        "flow visit(entries: PriorityQueue<string, i32>) -> unit { for (priority, value) in entries limit Iterations(8) { let wrong: string = priority; } }",
        "type Wrapped = OrderedMap<i32, i32>; flow visit(entries: Wrapped) -> unit { for pair in entries limit Iterations(8) {} }",
    ] {
        let output = check(source);
        assert!(!output.diagnostics.is_empty(), "{source}");
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| d.primary.span.source == SourceId(0))
        );
    }
}
