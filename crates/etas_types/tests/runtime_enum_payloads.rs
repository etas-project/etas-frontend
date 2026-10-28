use etas_types::{ItemSignature, Type, TypeId, substitute_named_params_in_store};

fn finalized(hir: &etas_hir::HirProgram) -> etas_types::TypeOutput {
    let signatures = etas_types::build_signature_facts(hir);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let base = etas_types::SignatureEnvironment::new(hir, &signatures);
    let registry = std::sync::Arc::new(etas_std::standard_registry());
    let bodies = hir
        .items
        .iter()
        .filter(|(_, item)| matches!(item, etas_hir::HirItem::Flow(_)))
        .map(|(item, _)| etas_types::check_body_delta(hir, &base, item, registry.clone()))
        .collect::<Vec<_>>();
    for body in &bodies {
        assert!(body.diagnostics.is_empty(), "{:?}", body.diagnostics);
    }
    etas_types::finalize_body_deltas(&base, &bodies).unwrap_or_else(|error| {
        panic!(
            "{error}; enum declarations: {:?}; layouts: {:?}",
            signatures
                .store
                .iter()
                .filter_map(|(id, data)| match data {
                    Type::Enum(value) => Some((id, &value.name)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            signatures.facts.enum_layouts.keys().collect::<Vec<_>>()
        )
    })
}

#[test]
fn finalization_materializes_nested_generic_enum_payloads_from_checked_layouts() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(11),
        None,
        r#"
module app.main;
type Receipt<T> = { value: T, history: Array<T> }
enum Outcome<T, E> { Committed(T), Rejected(E) }
enum Reconciled<T, E> { Found(Outcome<T, E>), Unresolved }
enum Envelope<T> { Wrapped(Option<Array<Reconciled<T, i32>>>) }
flow main(value: Envelope<Receipt<string>>) -> Envelope<Receipt<string>> { return value; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = finalized(&etas_hir::lower_program(&parsed.value));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let root = output
        .facts
        .item_signatures
        .values()
        .find_map(|signature| match signature {
            ItemSignature::Flow(flow) if flow.params.len() == 1 => Some(flow.output),
            _ => None,
        })
        .unwrap();
    let payload = |owner: TypeId, variant: &str| {
        let Type::Applied { constructor, args } = output.store.get(owner).unwrap() else {
            panic!("applied enum owner");
        };
        let layout = &output.facts.enum_layouts[&TypeId(constructor.0)];
        let fields = &layout
            .variants
            .iter()
            .find(|item| item.name == variant)
            .unwrap()
            .fields;
        assert_eq!(fields.len(), 1);
        let bindings = layout
            .type_params
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        substitute_named_params_in_store(&output.store, fields[0], &bindings).unwrap()
    };
    let wrapped = payload(root, "Wrapped");
    let Some(Type::Option(array)) = output.store.get(wrapped) else {
        panic!("checked option payload");
    };
    let Some(Type::Array(reconciled)) = output.store.get(*array) else {
        panic!("checked array payload");
    };
    let outcome = payload(*reconciled, "Found");
    let receipt = payload(outcome, "Committed");
    let representation = etas_types::record_fields_with_applied_params(&output.store, receipt)
        .unwrap()
        .unwrap();
    assert!(matches!(
        output.store.get(representation.fields[0].ty),
        Some(Type::Primitive(etas_types::PrimitiveType::String))
    ));
    assert!(
        matches!(output.store.get(representation.fields[1].ty), Some(Type::Array(inner)) if matches!(output.store.get(*inner), Some(Type::Primitive(etas_types::PrimitiveType::String))))
    );
}

#[test]
fn payload_materialization_preserves_recursive_declaration_templates() {
    for declarations in [
        "enum Tree<T> { Leaf(T), Children(List<Tree<T>>) }",
        "enum Tree<T> { Leaf(T), Next(Tree<List<T>>) }",
        "enum Tree<T> { Leaf(T), Next(Branch<T>) } enum Branch<T> { Back(Tree<T>) }",
    ] {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            etas_core::SourceId(13),
            None,
            format!(
                "module app.main; {declarations} flow main(value: Tree<i32>) -> Tree<i32> {{ return value; }}"
            ),
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let output = finalized(&etas_hir::lower_program(&parsed.value));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert!(
            output
                .facts
                .enum_layouts
                .values()
                .any(|layout| layout.type_params == ["T"] && layout.variants.len() == 2)
        );
    }
}
