use std::sync::Arc;

use etas_core::{SourceFile, SourceId};
use etas_hir::{HirItem, HirProgram};
use etas_types::{SignatureEnvironment, Type, TypeId, TypeInterner, TypeStore};

fn parse(source: &str) -> HirProgram {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(0), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_hir::lower_program(&parsed.value)
}

#[test]
fn signature_type_base_is_shared_while_body_interning_is_local() {
    let hir = parse("flow f() -> i32 { return 1; }");
    let signatures = etas_types::build_signature_facts(&hir);
    let base = SignatureEnvironment::new(&hir, &signatures);
    let mut first = TypeInterner::from_store(TypeStore::with_base(base.type_base().clone()));
    let mut second = TypeInterner::from_store(TypeStore::with_base(base.type_base().clone()));
    assert!(
        first
            .store()
            .base()
            .unwrap()
            .ptr_eq(second.store().base().unwrap())
    );
    for (id, ty) in first.store().iter() {
        assert!(std::ptr::eq(ty, second.store().get(id).unwrap()));
    }
    let count = base.type_base().len();
    let a = first.intern(Type::Tuple(vec![TypeId(0); 3]));
    let b = second.intern(Type::Tuple(vec![TypeId(0); 4]));
    assert_eq!(a, TypeId(count as u32));
    assert_eq!(a, b, "body-local IDs may overlap and must be relocated");
    assert_ne!(first.store().get(a), second.store().get(b));
    assert_eq!(base.type_base().len(), count);
    assert_eq!(first.store().local_types().len(), 1);
    assert_eq!(second.store().local_types().len(), 1);
}

#[test]
fn body_deltas_preserve_recursive_generic_and_nominal_types_after_relocation() {
    let hir = parse(
        r#"
enum Tree<T> { Leaf(T), Pair(Tree<T>, Tree<T>), }
type Label = string;
flow identity<T>(x: T) -> T { return x; }
flow tree() -> Tree<i32> { return Tree.Pair(Tree.Leaf(1), Tree.Leaf(2)); }
flow labels() -> Array<Label> { return [identity(Label("a")), Label("b")]; }
flow tuple() -> (i32, string) { return (identity(1), identity("x")); }
"#,
    );
    let signatures = etas_types::build_signature_facts(&hir);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let base = SignatureEnvironment::new(&hir, &signatures);
    let registry = Arc::new(etas_std::standard_registry());
    let bodies = hir
        .items
        .iter()
        .filter(|(_, item)| matches!(item, HirItem::Flow(_)))
        .map(|(item, _)| etas_types::check_body_delta(&hir, &base, item, registry.clone()))
        .collect::<Vec<_>>();
    for body in &bodies {
        assert!(base.accepts(body));
        assert!(body.diagnostics.is_empty(), "{:?}", body.diagnostics);
        assert!(body.facts.item_signatures.is_empty());
        assert!(body.facts.spec_signatures.is_empty());
        assert!(body.facts.enum_layouts.is_empty());
        assert!(body.facts.action_signatures.is_empty());
    }
    let merged = etas_types::finalize_body_deltas(&base, &bodies).unwrap();
    let reference = etas_types::check_program(&hir);
    assert!(
        reference.diagnostics.is_empty(),
        "{:?}",
        reference.diagnostics
    );
    assert_eq!(
        merged.facts.expr_types.len(),
        reference.facts.expr_types.len()
    );
    for (expr, ty) in &reference.facts.expr_types {
        assert_eq!(
            etas_types::ty::display_type(&merged.store, merged.facts.expr_types[expr]),
            etas_types::ty::display_type(&reference.store, *ty),
            "expression {expr:?}"
        );
    }
    assert!(merged.store.base().unwrap().ptr_eq(base.type_base()));
}

#[test]
fn finalization_rejects_wrong_base_and_dangling_local_type_ids() {
    let hir = parse("flow f() -> i32 { return 1; }");
    let signatures = etas_types::build_signature_facts(&hir);
    let base = SignatureEnvironment::new(&hir, &signatures);
    let item = hir
        .items
        .iter()
        .find(|(_, item)| matches!(item, HirItem::Flow(_)))
        .unwrap()
        .0;
    let mut body =
        etas_types::check_body_delta(&hir, &base, item, Arc::new(etas_std::standard_registry()));
    body.base.0[0] ^= 1;
    assert!(
        etas_types::finalize_body_deltas(&base, [&body])
            .unwrap_err()
            .contains("different signature type base")
    );
    body.base = base.type_base().identity();
    *body.facts.expr_types.values_mut().next().unwrap() = TypeId(u32::MAX);
    assert!(
        etas_types::finalize_body_deltas(&base, [&body])
            .unwrap_err()
            .contains("missing type")
    );
}

#[test]
fn body_delta_enforces_shared_spec_bounds_and_nominal_assignment() {
    for source in [
        "spec Allowed; enum Box<T ~ Allowed> { Named { value: T }, } flow f() -> Box<i32> { return Box.Named { value = 42 }; }",
        "type Id = string; flow f() -> Id { return \"no\"; }",
    ] {
        let hir = parse(source);
        let signatures = etas_types::build_signature_facts(&hir);
        let base = SignatureEnvironment::new(&hir, &signatures);
        let item = hir
            .items
            .iter()
            .find(|(_, item)| matches!(item, HirItem::Flow(_)))
            .unwrap()
            .0;
        let body = etas_types::check_body_delta(
            &hir,
            &base,
            item,
            Arc::new(etas_std::standard_registry()),
        );
        assert!(!body.diagnostics.is_empty(), "{source}");
        assert!(
            body.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.primary.span.source == SourceId(0))
        );
    }
}
