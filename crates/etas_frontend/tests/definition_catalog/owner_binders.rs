use super::*;
use etas_code::descriptor::{GenericOwner, GenericParameterRef, TypeInfoRef};

fn parameter(
    graph: &etas_code::descriptor::DescriptorGraph,
    ty: TypeInfoRef,
) -> GenericParameterRef {
    let Some(TypeInfo::Parameter(parameter)) = graph.type_info(ty) else {
        panic!("parameter");
    };
    *parameter
}

#[test]
fn generic_impl_catalog_preserves_owner_and_shadowing_method_binders() {
    let source = include_str!("../fixtures/method_resolution/owner_catalog.es");
    let checked = check(&[source]);
    let catalog = catalog(&checked);
    let (id, _) = catalog
        .native_callables()
        .find(|(id, _)| {
            id.as_method()
                .is_some_and(|method| method.name() == "replace")
        })
        .unwrap();
    let method = id.as_method().unwrap();
    let graph = catalog.descriptors();
    let Some(TypeInfo::Function(signature)) =
        graph.type_info(catalog.callable(id).unwrap().signature)
    else {
        panic!("signature");
    };
    assert_eq!(signature.generics.len(), 1);
    let local = signature.generics[0];
    let Some(TypeInfo::Applied { arguments, .. }) = graph.type_info(signature.parameters[0].ty)
    else {
        panic!("receiver");
    };
    let owner = parameter(graph, arguments[0]);
    assert_ne!(owner, local);
    assert_eq!(
        graph.binder(owner).unwrap().owner,
        GenericOwner::Implementation(method.implementation().clone())
    );
    assert_eq!(
        graph.binder(local).unwrap().owner,
        GenericOwner::Method(method.clone())
    );
    assert_eq!(parameter(graph, signature.parameters[1].ty), local);
    let Some(TypeInfo::Applied { arguments, .. }) = graph.type_info(signature.output) else {
        panic!("result");
    };
    assert_eq!(parameter(graph, arguments[0]), local);

    let renamed = source
        .replace("impl Container<T>", "impl Container<Value>")
        .replace("flow get(self) -> T", "flow get(self) -> Value");
    let other = check(&[&renamed]);
    let other = super::catalog(&other);
    let other_signature = other.callable(id).unwrap().signature;
    assert_eq!(
        graph
            .type_fingerprint(catalog.callable(id).unwrap().signature)
            .unwrap(),
        other
            .descriptors()
            .type_fingerprint(other_signature)
            .unwrap()
    );
}
