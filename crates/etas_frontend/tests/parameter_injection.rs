use etas_frontend::{EntryPolicy, Frontend, ProjectInput, SourceInput};

#[test]
fn omitted_providers_are_conditional_and_survive_package_metadata_encoding() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"module injection;
public effect Supply { action get() -> i32; }
public flow value(@inject(Supply.get) dependency: i32) -> i32 ![] { return dependency; }
public flow explicit() -> i32 ![] { return value(42); }
public flow omitted() -> i32 ![Supply.get] { return value(); }
"#,
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let item = |name: &str| {
        checked
            .hir
            .items
            .iter()
            .find_map(|(item, declaration)| {
                let symbol = declaration.symbol()?;
                (checked.symbols.get(symbol)?.name == name).then_some(item)
            })
            .unwrap()
    };
    assert!(
        checked.effects.item_effects[&item("value")]
            .requested_actions
            .is_empty()
    );
    assert!(
        checked.effects.item_effects[&item("explicit")]
            .requested_actions
            .is_empty()
    );
    assert!(
        !checked.effects.item_effects[&item("omitted")]
            .requested_actions
            .is_empty()
    );
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(
                etas_code::PackageKey::from_name_and_version("injection", "0.1.0").unwrap(),
            ),
            package_id: "injection".into(),
            package_version: "0.1.0".into(),
            package_edition: "2026".into(),
            compiler_version: "test".into(),
            source_payload_hash: "a".repeat(64),
            manifest_hash: "b".repeat(64),
            dependency_lock_hash: "c".repeat(64),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &checked,
    )
    .unwrap();
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("package.etasm"),
        &artifact.bytes,
    )
    .unwrap();
    let default = &metadata
        .public_metadata
        .flows
        .iter()
        .find(|signature| signature.path == ["injection", "value"])
        .unwrap()
        .parameter_defaults[0];
    assert!(default.reference.injected);
    assert!(
        !default
            .binding
            .as_ref()
            .unwrap()
            .requested_actions
            .effects
            .is_empty()
    );
}

#[test]
fn hidden_tool_callable_results_keep_their_provider_handler_origins() {
    let package = etas_code::PackageKey::from_name_and_version("injection", "0.1.0").unwrap();
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        r#"module app.main;
import std.runtime.checkpoint;
effect Evidence { action read() -> (() -> string ![]); }
tool lookup(@inject(Evidence.read) dependency: () -> string ![], suffix: string = "!") -> string ![] {
    return dependency() + suffix;
}
@tools([lookup])
agent Reader() -> string ![Evidence.read] { return perform infer(Prompt.new()); }
flow main() -> string ![Agentic] {
    return Reader.run() with { Evidence.read() => { checkpoint("provider-binding"); () => "provided" }; };
}"#,
    ));
    input.environment.local_source_package = Some(package);
    let output = Frontend.check_project(input);
    assert!(
        output.diagnostics.iter().all(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::RuntimeRequiredInPhase1
            )),
        "{:?}",
        output.diagnostics
    );
    let checked = output.checked.unwrap();
    use etas_frontend::code::{definition::DefinitionCatalog, identity::DefinitionOrigins};
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: etas_frontend::code::identity::standard_package_key(
            &checked.std_registry,
        )
        .unwrap(),
        external_definitions: DefinitionOrigins::checked_external_definitions(&checked).unwrap(),
    };
    DefinitionCatalog::build(checked, &origins, Default::default()).unwrap();
}
