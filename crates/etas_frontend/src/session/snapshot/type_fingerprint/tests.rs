use super::*;
use etas_types::{FieldType, NominalTypeRef, PrimitiveType, RecordType};

fn recursive(offset: usize, value: PrimitiveType) -> (TypeStore, TypeId) {
    let mut types = TypeStore::new();
    for index in 0..offset {
        types.intern(Type::Named(etas_types::NamedTypeRef {
            name: format!("Unused{index}"),
            source_parameter: None,
        }));
    }
    let node = types.intern(Type::Nominal(NominalTypeRef {
        name: "app.Node".into(),
        params: vec![],
        representation: Some(TypeId(offset as u32 + 3)),
    }));
    let value = types.intern(Type::Primitive(value));
    let next = types.intern(Type::Option(node));
    let record = types.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "value".into(),
                ty: value,
            },
            FieldType {
                name: "next".into(),
                ty: next,
            },
        ],
    }));
    assert_eq!(record, TypeId(offset as u32 + 3));
    (types, node)
}

#[test]
fn recursive_type_fingerprints_are_finite_and_arena_independent() {
    let (first, a) = recursive(0, PrimitiveType::I32);
    let (second, b) = recursive(13, PrimitiveType::I32);
    assert_ne!(a, b);
    assert_eq!(
        type_part(a, &first).unwrap(),
        type_part(b, &second).unwrap()
    );
}

#[test]
fn representation_changes_invalidate_a_recursive_signature() {
    let (first, a) = recursive(0, PrimitiveType::I32);
    let (second, b) = recursive(0, PrimitiveType::String);
    assert_ne!(
        type_part(a, &first).unwrap(),
        type_part(b, &second).unwrap()
    );
}

#[test]
fn deep_signatures_use_a_worklist_instead_of_the_rust_call_stack() {
    let mut types = TypeStore::new();
    let mut root = types.intern(Type::Primitive(PrimitiveType::I32));
    for _ in 0..10_000 {
        root = types.intern(Type::Option(root));
    }
    assert!(type_part(root, &types).unwrap().starts_with("type-graph:"));
}

#[test]
fn effect_row_tail_participates_in_the_signature_fingerprint() {
    let row = |tail: &str| {
        Some(EffectRowRef {
            effects: vec![],
            tail: Some(tail.into()),
        })
    };
    let store = TypeStore::new();
    assert_ne!(
        effect_row_part(&row("E"), &store).unwrap(),
        effect_row_part(&row("F"), &store).unwrap()
    );
    assert_ne!(
        effect_row_part(&None, &store).unwrap(),
        effect_row_part(
            &Some(EffectRowRef {
                effects: vec![],
                tail: None
            }),
            &store
        )
        .unwrap()
    );
}

#[test]
fn applied_constructors_use_identity_not_arena_numbers() {
    let applied = |offset, name: &str| {
        let (mut types, _) = recursive(offset, PrimitiveType::I32);
        let constructor = types.intern(Type::Nominal(NominalTypeRef {
            name: name.into(),
            params: vec!["T".into()],
            representation: None,
        }));
        let arg = types.intern(Type::Primitive(PrimitiveType::Bool));
        let applied = types.intern(Type::Applied {
            constructor: TypeConstructorId(constructor.0),
            args: vec![arg],
        });
        type_part(applied, &types).unwrap()
    };
    assert_eq!(applied(0, "app.Box"), applied(8, "app.Box"));
    assert_ne!(applied(0, "app.Box"), applied(0, "app.Other"));
}

#[test]
fn missing_checked_types_are_errors_not_panics_or_placeholder_hashes() {
    let error = type_part(TypeId(9), &TypeStore::new()).unwrap_err();
    assert!(error.contains("missing type 9"));
}
