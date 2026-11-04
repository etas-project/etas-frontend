use super::*;

#[test]
fn in_memory_method_metadata_must_reference_its_checked_descriptor_binders() {
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
    let public = metadata.public_metadata;
    let package = ExternalPackageId(13);
    let mut environment = ProjectEnvironmentInput {
        external_definition_origins: vec![crate::ProjectExternalDefinitionOriginsInput {
            package,
            origins: public.definition_origins.unwrap(),
            descriptors: public.definition_descriptors,
        }],
        external_public_metadata: vec![crate::ProjectExternalPublicMetadataInput {
            package,
            methods: public.methods,
            types: vec![],
            values: vec![],
            enums: vec![],
            flows: vec![],
            agents: vec![],
            tools: vec![],
            tool_schemas: vec![],
            effects: vec![],
            actions: vec![],
            trace_specs: vec![],
            spec_signatures: vec![],
            spec_impls: vec![],
            type_spec_satisfactions: vec![],
            callable_spec_satisfactions: vec![],
            trace_spec_conformances: vec![],
            effect_summaries: vec![],
            action_summaries: vec![],
            trace_spec_summaries: vec![],
            re_exports: vec![],
        }],
        ..Default::default()
    };
    validate(&environment).unwrap();
    let method = environment.external_public_metadata[0]
        .methods
        .iter_mut()
        .find(|method| method.identity.name() == "replace")
        .unwrap();
    method.self_type.children[0]
        .parameter
        .as_mut()
        .unwrap()
        .position = 1;
    method.callable.values[0].params[0] = method.self_type.clone();
    let error = validate(&environment).unwrap_err();
    assert_eq!(error.package, Some(package));
    assert!(error.reason.contains("declared type binder"), "{error:?}");
}
