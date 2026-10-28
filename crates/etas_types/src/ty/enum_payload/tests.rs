use super::*;
use crate::{EnumTypeRef, EnumVariantLayoutFact, NamedTypeRef, PrimitiveType, TypeConstructorId};

fn layout(field: TypeId) -> EnumLayoutFact {
    EnumLayoutFact {
        type_params: vec!["T".into()],
        variants: vec![EnumVariantLayoutFact {
            name: "Value".into(),
            fields: vec![field],
            field_names: None,
        }],
    }
}

#[test]
fn payload_graph_and_instantiation_use_worklists_for_long_acyclic_chains() {
    let mut interner = TypeInterner::new();
    let param = interner.intern(Type::Named(NamedTypeRef {
        name: "T".into(),
        source_parameter: None,
    }));
    let integer = interner.primitive(PrimitiveType::I32);
    let mut layouts = HashMap::new();
    let mut owners = Vec::new();
    let mut payload = param;
    for index in 0..2_000 {
        let owner = interner.intern(Type::Enum(EnumTypeRef {
            name: format!("Chain{index}"),
        }));
        layouts.insert(owner, layout(payload));
        owners.push(owner);
        payload = interner.intern(Type::Applied {
            constructor: TypeConstructorId(owner.0),
            args: vec![param],
        });
    }
    let root = interner.intern(Type::Applied {
        constructor: TypeConstructorId(owners.last().unwrap().0),
        args: vec![integer],
    });
    let mut materializer =
        super::super::representation::TypeRepresentationMaterializer::with_acyclic_enum_payloads(
            interner.store(),
            &layouts,
        )
        .unwrap();
    materializer.materialize(&mut interner, [root]).unwrap();
    let index = crate::ty::TypeLookupIndex::new(interner.store());
    for owner in owners {
        assert!(
            index
                .get(
                    interner.store(),
                    &Type::Applied {
                        constructor: TypeConstructorId(owner.0),
                        args: vec![integer]
                    }
                )
                .is_some()
        );
    }
}

#[test]
fn payload_graph_rejects_dangling_types_and_missing_dependency_layouts() {
    let mut interner = TypeInterner::new();
    let owner = interner.intern(Type::Enum(EnumTypeRef {
        name: "Owner".into(),
    }));
    let mut layouts = HashMap::from([(owner, layout(TypeId(u32::MAX)))]);
    assert!(matches!(
        AcyclicEnumPayloads::new(interner.store(), &layouts),
        Err(TypeSubstitutionError::MissingType(TypeId(u32::MAX)))
    ));
    let missing = interner.intern(Type::Enum(EnumTypeRef {
        name: "Missing".into(),
    }));
    layouts.insert(owner, layout(missing));
    assert!(
        matches!(AcyclicEnumPayloads::new(interner.store(), &layouts), Err(TypeSubstitutionError::MissingEnumLayout(ty)) if ty == missing)
    );
}

#[test]
fn recursive_payloads_are_explicit_templates_not_empty_payload_facts() {
    let mut interner = TypeInterner::new();
    let owner = interner.intern(Type::Enum(EnumTypeRef {
        name: "Growing".into(),
    }));
    let param = interner.intern(Type::Named(NamedTypeRef {
        name: "T".into(),
        source_parameter: None,
    }));
    let list = interner.intern(Type::List(param));
    let field = interner.intern(Type::Applied {
        constructor: TypeConstructorId(owner.0),
        args: vec![list],
    });
    let integer = interner.primitive(PrimitiveType::I32);
    let layouts = HashMap::from([(owner, layout(field))]);
    let payloads = AcyclicEnumPayloads::new(interner.store(), &layouts).unwrap();
    let count = interner.store().len();
    assert!(matches!(
        payloads
            .expand(&mut interner, owner, &[integer], true)
            .unwrap(),
        EnumPayloadExpansion::RecursiveTemplate
    ));
    assert_eq!(interner.store().len(), count);
    assert_eq!(layouts[&owner].variants[0].fields, [field]);
    assert!(
        matches!(payloads.expand(&mut interner, owner, &[], true), Err(TypeSubstitutionError::InvalidApplication(ty)) if ty == owner)
    );
    assert!(matches!(
        payloads.expand(&mut interner, owner, &[], false).unwrap(),
        EnumPayloadExpansion::GenericDeclaration
    ));
}
