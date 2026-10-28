use super::*;

fn projected(source: &str, name: &str) -> MetadataType {
    let output = crate::Frontend.check_project(crate::ProjectInput::single_source(
        crate::SourceInput::anonymous(source),
    ));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let ty = checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            matches!(ty, Type::Nominal(nominal) if nominal.name == name).then_some(id)
        })
        .unwrap();
    metadata_type(ty, &checked.type_store).unwrap()
}

#[test]
fn direct_nominal_recursion_emits_a_canonical_back_reference() {
    let metadata = projected(
        "module app; type Node = { next: Option<Node> }; flow main() -> unit { return; }",
        "app.Node",
    );
    assert_eq!(metadata.kind, MetadataTypeKind::Nominal);
    let next = &metadata.children[0].fields[0].ty.children[0];
    assert_eq!(next.kind, MetadataTypeKind::Named);
    assert_eq!(next.path, ["app", "Node"]);
    assert!(next.children.is_empty());
}

#[test]
fn transparent_structural_aliases_can_cross_a_nominal_recursion_boundary() {
    let metadata = projected(
        "module app; alias Link = { next: Option<Node> }; type Node = Link; flow main() -> unit { return; }",
        "app.Node",
    );
    assert_eq!(metadata.kind, MetadataTypeKind::Nominal);
    assert_eq!(
        metadata.children[0].fields[0].ty.children[0].kind,
        MetadataTypeKind::Named
    );
}

#[test]
fn mutually_recursive_nominals_preserve_both_identities() {
    let metadata = projected(
        "module app; type Directory = { entry: Option<Entry> }; type Entry = { parent: Directory }; flow main() -> unit { return; }",
        "app.Directory",
    );
    let entry = &metadata.children[0].fields[0].ty.children[0];
    assert_eq!(entry.kind, MetadataTypeKind::Nominal);
    assert_eq!(entry.path, ["app", "Entry"]);
    let parent = &entry.children[0].fields[0].ty;
    assert_eq!(parent.kind, MetadataTypeKind::Named);
    assert_eq!(parent.path, ["app", "Directory"]);
}

#[test]
fn recursive_effect_type_arguments_use_the_same_projection_context() {
    let metadata = projected(
        "module app; type Node = { visit: () -> unit ![Error<Node>] }; flow main() -> unit { return; }",
        "app.Node",
    );
    let visit = &metadata.children[0].fields[0].ty;
    let arg = visit.effects.as_ref().unwrap().effects[0].args[0]
        .ty
        .as_ref()
        .unwrap();
    assert_eq!(arg.kind, MetadataTypeKind::Named);
    assert_eq!(arg.path, ["app", "Node"]);
}

#[test]
fn structural_cycles_without_nominal_identity_fail_closed() {
    let mut store = TypeStore::new();
    let root = store.intern(Type::Option(TypeId(0)));
    assert_eq!(root, TypeId(0));
    let error = metadata_type(root, &store).unwrap_err();
    assert!(error.to_string().contains("nominal identity boundary"));
}

#[test]
fn deep_acyclic_types_reach_the_explicit_metadata_budget() {
    let mut store = TypeStore::new();
    let mut root = store.intern(Type::Primitive(PrimitiveType::I32));
    for _ in 0..etas_package_metadata::MAX_METADATA_GRAPH_DEPTH + 2 {
        root = store.intern(Type::Option(root));
    }
    let error = metadata_type(root, &store).unwrap_err();
    assert!(error.to_string().contains("metadata graph budget"));
}
