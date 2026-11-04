use super::*;
use etas_types::{
    ExternalCallableGenericParamInput as Param, ExternalCallableGenericParamKindInput as Kind,
};

#[test]
fn binary_owner_binders_lower_and_solve_a_shadowing_method_chain() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/method_resolution/owner_catalog.es"
    ))
    .replace("type Container", "public type Container");
    let output = crate::Frontend.check_project(crate::ProjectInput::single_source(
        crate::SourceInput::anonymous(source),
    ));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let artifact = crate::build_package_metadata_artifact_from_checked(
        crate::PackageMetadataBuildInput {
            definition_origin: Some(
                etas_code::PackageKey::from_name_and_version("library", "1.0.0").unwrap(),
            ),
            package_id: "library".into(),
            package_version: "1.0.0".into(),
            package_edition: "2026".into(),
            compiler_version: crate::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: "source".into(),
            manifest_hash: "manifest".into(),
            dependency_lock_hash: "lock".into(),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &output.checked.unwrap(),
    )
    .unwrap();
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("library.etasmeta"),
        &artifact.bytes,
    )
    .unwrap();
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        "module consumer; import app.Container; flow main() -> string ![] { return Container<i32> { value = 7 }.replace(\"changed\").get(); }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let import = hir
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Container")
        .unwrap();
    let mut input = etas_types::SignaturePipelineInput::new(&hir);
    let registry = input.std_registry.clone();
    let package = etas_types::ExternalPackageKey(1);
    input
        .external
        .package_anchors
        .push((package, import.definition_span));
    input
        .external
        .symbol_bindings
        .push(etas_types::ExternalSymbolBindingInput {
            symbol: import.id,
            package,
            path: vec!["app".into(), "Container".into()],
            span: import.definition_span,
        });
    let published = metadata.public_metadata;
    let behavior_types = published
        .methods
        .iter()
        .flat_map(|method| {
            let root = method.root().unwrap();
            std::iter::once(&method.self_type)
                .chain(&root.params)
                .chain(std::iter::once(&root.output))
        })
        .map(|ty| package_type_input(ty).unwrap())
        .collect();
    input
        .external
        .metadata
        .push(etas_types::ExternalPublicMetadataInput {
            package,
            behavior_types,
            types: published
                .types
                .iter()
                .map(|decl| etas_types::ExternalNamedSignatureInput {
                    path: decl.path.clone(),
                    generic_params: decl
                        .generic_params
                        .iter()
                        .map(|parameter| {
                            assert!(parameter.bounds.is_empty());
                            Param {
                                name: parameter.name.clone(),
                                kind: Kind::Type,
                                bounds: vec![],
                            }
                        })
                        .collect(),
                    ty: decl.ty.as_ref().map(|ty| package_type_input(ty).unwrap()),
                })
                .collect(),
            methods: published
                .methods
                .iter()
                .map(|method| signature(method).unwrap())
                .collect(),
            values: vec![],
            enums: vec![],
            flows: vec![],
            agents: vec![],
            tools: vec![],
            effects: vec![],
            trace_specs: vec![],
            spec_signatures: vec![],
            spec_impls: vec![],
            type_spec_satisfactions: vec![],
            callable_spec_satisfactions: vec![],
            trace_spec_conformances: vec![],
            actions: vec![],
        });
    let signatures = etas_types::run_signature_pipeline(input);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let replace = signatures
        .facts
        .external_methods
        .iter()
        .find(|method| method.identity.name() == "replace")
        .unwrap();
    assert_ne!(
        replace.owner_params[0].subject,
        replace.signature.generic_params[0].subject
    );
    let identity = signatures
        .store
        .parameter_identity(replace.owner_params[0].subject)
        .unwrap();
    assert_eq!(
        signatures.store.canonical_parameter(identity),
        Some(replace.owner_params[0].subject)
    );
    let base = etas_types::SignatureEnvironment::new(&hir, &signatures);
    let main = hir
        .items
        .iter()
        .find_map(|(id, item)| matches!(item, etas_hir::HirItem::Flow(_)).then_some(id))
        .unwrap();
    let body = etas_types::check_body_delta(&hir, &base, main, registry);
    assert!(body.diagnostics.is_empty(), "{:?}", body.diagnostics);
    assert_eq!(body.facts.checked_method_calls.len(), 2);
    for fact in body.facts.checked_method_calls.values() {
        assert!(matches!(
            fact.target,
            etas_types::method::MethodTarget::External { .. }
        ));
        assert_eq!(fact.owner_bindings.len(), 1);
    }
}
