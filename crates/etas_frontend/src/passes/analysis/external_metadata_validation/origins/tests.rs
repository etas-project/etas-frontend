use super::*;
use etas_code::{CodeFingerprint, PackageKey};

fn key(name: &str) -> DefinitionKey {
    DefinitionKey::new(
        PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([1; 32])),
        vec!["library".into()],
        vec![name.into()],
        DefinitionKind::Flow,
    )
    .unwrap()
}

fn path(name: &str) -> Vec<String> {
    vec!["library".into(), name.into()]
}

fn binding<'a>(
    definition: &'a DefinitionKey,
    source: Option<&'a [String]>,
    direct: bool,
) -> Binding<'a> {
    Binding {
        package: ExternalPackageId(7),
        definition,
        source,
        direct,
    }
}

#[test]
fn aliases_must_terminate_at_a_checked_declaration() {
    let identity = key("origin");
    let (origin, middle, alias) = (path("origin"), path("middle"), path("alias"));
    let bindings = BTreeMap::from([
        (origin.clone(), binding(&identity, None, true)),
        (middle.clone(), binding(&identity, Some(&origin), false)),
        (alias, binding(&identity, Some(&middle), false)),
    ]);
    validate_chains(&bindings).unwrap();
}

#[test]
fn same_identity_does_not_validate_a_re_export_cycle() {
    let identity = key("origin");
    let (a, b) = (path("a"), path("b"));
    let bindings = BTreeMap::from([
        (a.clone(), binding(&identity, Some(&b), false)),
        (b.clone(), binding(&identity, Some(&a), false)),
    ]);
    assert!(
        validate_chains(&bindings)
            .unwrap_err()
            .reason
            .contains("cyclic")
    );
}

#[test]
fn verified_target_does_not_hide_an_identity_change() {
    let identity = key("origin");
    let forged = key("forged");
    let origin = path("a");
    let bindings = BTreeMap::from([
        (origin.clone(), binding(&identity, None, true)),
        (path("z"), binding(&forged, Some(&origin), false)),
    ]);
    assert!(
        validate_chains(&bindings)
            .unwrap_err()
            .reason
            .contains("changes canonical identity")
    );
}

#[test]
fn absent_and_signatureless_targets_are_not_declarations() {
    let identity = key("origin");
    let missing = path("missing");
    let bindings = BTreeMap::from([(path("a"), binding(&identity, Some(&missing), false))]);
    assert!(
        validate_chains(&bindings)
            .unwrap_err()
            .reason
            .contains("no canonical target binding")
    );
    let bindings = BTreeMap::from([(path("a"), binding(&identity, None, false))]);
    assert!(
        validate_chains(&bindings)
            .unwrap_err()
            .reason
            .contains("no checked canonical declaration")
    );
}

#[test]
fn long_alias_chains_are_iterative_and_have_no_fixed_depth_fallback() {
    let identity = key("origin");
    let paths = (0..5000)
        .map(|i| path(&format!("item{i:05}")))
        .collect::<Vec<_>>();
    let mut bindings = BTreeMap::new();
    for (i, path) in paths.iter().enumerate() {
        bindings.insert(
            path.clone(),
            binding(
                &identity,
                paths.get(i + 1).map(Vec::as_slice),
                i + 1 == paths.len(),
            ),
        );
    }
    validate_chains(&bindings).unwrap();
}

#[test]
fn graph_budget_is_shared_and_checked_before_building_identity_indexes() {
    use etas_code::identity::{DefinitionModuleOrigin, PackageDefinitionOrigins};
    let mut environment = ProjectEnvironmentInput::default();
    for package in [ExternalPackageId(7), ExternalPackageId(8)] {
        environment.external_definition_origins.push(
            crate::ProjectExternalDefinitionOriginsInput {
                package,
                descriptors: None,
                origins: PackageDefinitionOrigins {
                    package: key("origin").package(),
                    modules: vec![
                        DefinitionModuleOrigin {
                            path: vec!["module".into()],
                            canonical_path: vec!["module".into()]
                        };
                        18_000
                    ],
                    declarations: Vec::new(),
                },
            },
        );
    }
    let error = validate(
        &environment,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert_eq!(error.package, Some(ExternalPackageId(8)));
    assert!(error.reason.contains("node limit"), "{error:?}");
}
