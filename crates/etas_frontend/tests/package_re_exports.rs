use etas_frontend::{
    Frontend, PackageMetadataBuildInput, ProjectInput, SourceInput, build_package_metadata_artifact,
};

fn publish(imports: &str) -> etas_package_metadata::PublicMetadata {
    let source = format!("module facade.api; {imports}");
    let mut input = ProjectInput::single_source(SourceInput::anonymous(&source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let artifact = build_package_metadata_artifact(
        PackageMetadataBuildInput {
            definition_origin: None,
            package_id: "facade".into(),
            package_version: "0.1.0".into(),
            package_edition: "2026".into(),
            compiler_version: "0.1.0".into(),
            source_payload_hash: etas_package_metadata::blake3_hash(source.as_bytes()),
            manifest_hash: "0".repeat(64),
            dependency_lock_hash: "1".repeat(64),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &output,
    )
    .unwrap();
    etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("facade.etasm"),
        &artifact.bytes,
    )
    .unwrap()
    .1
    .public_metadata
}

#[test]
fn facade_fingerprint_changes_when_a_forwarded_target_changes() {
    let old = publish("public import std.text.trim as normalize;");
    let new = publish("public import std.text.uppercase as normalize;");
    assert_eq!(old.re_exports.len(), 1);
    assert_eq!(new.re_exports.len(), 1);
    assert_eq!(old.re_exports[0].exported, new.re_exports[0].exported);
    assert_ne!(old.re_exports[0].from, new.re_exports[0].from);
    assert_ne!(old.fingerprint, new.fingerprint);
}

#[test]
fn facade_fingerprint_is_independent_of_import_declaration_order() {
    let old = publish(
        "public import std.text.trim as normalize; public import std.text.uppercase as shout;",
    );
    let new = publish(
        "public import std.text.uppercase as shout; public import std.text.trim as normalize;",
    );
    assert_eq!(old.re_exports, new.re_exports);
    assert_eq!(old.fingerprint, new.fingerprint);
}
