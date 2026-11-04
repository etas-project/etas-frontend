use super::*;

#[test]
fn same_spelled_source_parameter_cannot_replace_a_checked_descriptor_binder() {
    let checked = checked(
        "module app; flow first<T>(value: T) -> T { return value; }
         flow second<T>(value: T) -> T { return value; }",
    );
    let identities =
        DefinitionIdentityIndex::build(&checked, &origins(&checked, "origin:a")).unwrap();
    let first = flow(&checked, "first");
    let second = flow(&checked, "second");
    let HirItem::Flow(first_decl) = &checked.items[first] else {
        panic!("first flow");
    };
    let ItemSignature::Flow(first_signature) = &checked.types.item_signatures[&first] else {
        panic!("first signature");
    };
    let ItemSignature::Flow(second_signature) = &checked.types.item_signatures[&second] else {
        panic!("second signature");
    };
    assert_ne!(
        first_signature.generic_params[0].subject,
        second_signature.generic_params[0].subject
    );
    let mut changed = first_signature.clone();
    changed.generic_params[0].subject = second_signature.generic_params[0].subject;
    let mut projector =
        DefinitionDescriptorProjector::new(&checked, &identities, DescriptorLimits::default());
    projector
        .project_signature(
            GenericOwner::Declaration(identities.symbol(first_decl.symbol).unwrap().clone()),
            &changed,
            &[Some("value".into())],
            None,
        )
        .unwrap();
    assert!(matches!(
        projector.finish(),
        Err(ProjectionError::MissingIdentity(_))
    ));
}
