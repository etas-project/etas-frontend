use etas_types::{PrimitiveType, Type, TypeInterner, TypeStore, ty::SharedTypeBase};

#[test]
fn empty_tuple_interns_as_the_unit_type() {
    let mut types = TypeInterner::new();
    let unit = types.primitive(PrimitiveType::Unit);
    assert_eq!(types.intern(Type::Tuple(Vec::new())), unit);
    assert_eq!(
        types.store().get(unit),
        Some(&Type::Primitive(PrimitiveType::Unit))
    );

    let base = SharedTypeBase::new(types.into_store());
    let mut body = TypeInterner::from_store(TypeStore::with_base(base));
    assert_eq!(body.intern(Type::Tuple(Vec::new())), unit);
    assert!(body.store().local_types().is_empty());
}

#[test]
fn raw_and_serialized_type_stores_keep_unit_canonical() {
    let mut store = TypeStore::new();
    let id = store.intern(Type::Tuple(Vec::new()));
    assert_eq!(store.get(id), Some(&Type::Primitive(PrimitiveType::Unit)));
    let wire = serde_json::json!({ "types": [Type::Tuple(Vec::new())] });
    let decoded: TypeStore = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded.get(id), store.get(id));
}
