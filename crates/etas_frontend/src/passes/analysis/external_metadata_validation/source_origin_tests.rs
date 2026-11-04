use super::validate_source_package_bindings;
use crate::{ProjectInput, ProjectStandaloneSourcePackage, SourceInput, SourceKind};
use etas_code::{CodeFingerprint, PackageKey};
use etas_core::SourceId;

fn validate(input: &ProjectInput) -> Result<(), super::SourcePackageBindingError> {
    validate_source_package_bindings(
        &input.environment,
        input.sources.iter().map(|source| (source.id, &source.kind)),
    )
}

#[test]
fn source_package_bindings_reject_stale_conflicting_and_nonlocal_entries() {
    let source = SourceInput::anonymous("module app.main;");
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([1; 32]));
    let binding = ProjectStandaloneSourcePackage {
        source: source.id,
        package,
    };
    let mut input = ProjectInput::single_source(source);
    input.environment.standalone_source_packages = vec![binding.clone()];
    assert!(validate(&input).is_ok());
    input
        .environment
        .standalone_source_packages
        .push(binding.clone());
    assert_eq!(
        validate(&input).err().unwrap().reason,
        "source package identity is duplicated"
    );
    input.environment.standalone_source_packages.pop();
    input.environment.local_source_package = Some(package);
    assert_eq!(
        validate(&input).err().unwrap().reason,
        "source package identity conflicts with the local project package"
    );
    input.environment.local_source_package = None;
    input.sources[0].kind = SourceKind::VirtualGenerated;
    assert_eq!(
        validate(&input).err().unwrap().reason,
        "source package identity is attached to a non-local source"
    );
    input.sources[0].kind = SourceKind::SingleFileInput;
    input.environment.standalone_source_packages[0].source = SourceId(42);
    let error = validate(&input).err().unwrap();
    assert_eq!(error.source, SourceId(42));
    assert_eq!(
        error.reason,
        "source package identity refers to a missing source"
    );
}

#[test]
fn module_parts_require_one_consistent_package_when_origins_are_supplied() {
    let first = SourceInput::anonymous("module app.main; flow first() -> i32 { return 1; }");
    let mut second = SourceInput::anonymous("module app.main; flow second() -> i32 { return 2; }");
    second.id = SourceId(1);
    let package =
        |byte| PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([byte; 32]));
    let mut input = ProjectInput::single_source(first.clone());
    input.sources.push(second.clone());
    input.options.entry_policy = crate::EntryPolicy::Optional;

    let check = |input: &ProjectInput| crate::Frontend.check_project(input.clone());
    assert!(check(&input).diagnostics.is_empty());
    input.environment.standalone_source_packages = vec![
        ProjectStandaloneSourcePackage {
            source: first.id,
            package: package(1),
        },
        ProjectStandaloneSourcePackage {
            source: second.id,
            package: package(1),
        },
    ];
    let result = check(&input);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);

    input.environment.standalone_source_packages[1].package = package(2);
    let result = check(&input);
    assert!(result.checked.is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("conflicting package identities")
                && diagnostic.primary.span.source == second.id
        }),
        "{:?}",
        result.diagnostics
    );

    input.environment.standalone_source_packages.pop();
    let result = check(&input);
    assert!(result.checked.is_none());
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("no package identity")
                && diagnostic.primary.span.source == second.id
        }),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn dependency_source_origins_are_checked_against_package_and_metadata() {
    use crate::{
        ExternalPackageId, ProjectDependencySourcePackage, ProjectEnvironmentInput,
        ProjectExternalDefinitionOriginsInput, ProjectExternalPackageInput,
    };
    let id = ExternalPackageId(3);
    let owner = PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([1; 32]));
    let other = PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([2; 32]));
    let mut environment = ProjectEnvironmentInput::default();
    environment
        .external_packages
        .push(ProjectExternalPackageInput {
            id,
            name: "dependency".into(),
            version: "1".into(),
            edition: "2026".into(),
            import_root: "dep".into(),
            std_compatibility: etas_std::standard_registry()
                .version()
                .compatibility_identity(),
        });
    let binding = ProjectDependencySourcePackage {
        package: id,
        import_root: "dep".into(),
        definition_package: owner,
    };
    environment.dependency_source_packages.push(binding.clone());
    let mut source = SourceInput::anonymous("module dep.api;");
    source.kind = SourceKind::DependencySourceOverlay {
        package: id,
        import_root: "dep".into(),
    };
    assert_eq!(environment.source_package(&source), Ok(owner));
    let registry = etas_std::standard_registry();
    assert!(super::validate_external_environment(&environment, &registry).is_ok());
    let fingerprint = environment.canonical_environment_fingerprint();
    environment.dependency_source_packages[0].definition_package = other;
    assert_ne!(fingerprint, environment.canonical_environment_fingerprint());
    environment.dependency_source_packages[0] = binding.clone();
    environment.dependency_source_packages.push(binding.clone());
    assert!(
        super::validate_external_environment(&environment, &registry)
            .err()
            .unwrap()
            .reason
            .contains("duplicated")
    );
    environment.dependency_source_packages.pop();
    environment.dependency_source_packages[0].import_root = "wrong".into();
    assert!(
        super::validate_external_environment(&environment, &registry)
            .err()
            .unwrap()
            .reason
            .contains("mismatched import root")
    );
    environment.dependency_source_packages[0] = binding;
    environment
        .external_definition_origins
        .push(ProjectExternalDefinitionOriginsInput {
            package: id,
            origins: etas_code::identity::PackageDefinitionOrigins {
                package: other,
                modules: vec![],
                declarations: vec![],
            },
            descriptors: None,
        });
    assert!(
        super::validate_external_environment(&environment, &registry)
            .err()
            .unwrap()
            .reason
            .contains("conflicts")
    );
}
