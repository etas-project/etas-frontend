use std::collections::BTreeMap;

use etas_code::{CodeFingerprint, MethodId, PackageKey};
use etas_frontend::{
    EntryPolicy, Frontend, ProjectInput, SourceInput,
    code::{definition::DefinitionCatalog, identity::DefinitionOrigins},
};

fn checked(source: &str) -> etas_frontend::CheckedProgram {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn package() -> PackageKey {
    PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.type-impl-package",
        b"package",
    ))
}

fn all_methods(source: &str) -> Vec<MethodId> {
    let checked = checked(source);
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package()).unwrap(),
        standard_package: etas_frontend::code::identity::standard_package_key(
            &checked.std_registry,
        )
        .unwrap(),
        external_definitions: DefinitionOrigins::checked_external_definitions(&checked).unwrap(),
    };
    let catalog = DefinitionCatalog::build(checked, &origins, Default::default()).unwrap();
    catalog
        .native_callables()
        .filter_map(|(identity, _)| identity.as_method().cloned())
        .collect()
}

fn methods(source: &str) -> BTreeMap<String, MethodId> {
    all_methods(source)
        .into_iter()
        .map(|method| {
            (
                method.implementation().subject().declaration().join("."),
                method,
            )
        })
        .collect()
}

#[test]
fn two_nominal_implementations_have_distinct_method_identities() {
    let ids = methods(
        r#"
module library.api;
spec Encoder { flow encode(self) -> i32 ![]; }
type First = { value: i32 };
type Second = { value: i32 };
impl First ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }
impl Second ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }
"#,
    );
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert_ne!(ids["First"], ids["Second"]);
    assert_eq!(ids["First"].name(), "encode");
    assert_eq!(ids["Second"].name(), "encode");
}

#[test]
fn impl_identity_ignores_method_bodies_and_unrelated_item_order() {
    let source = r#"
module library.api;
spec Encoder { flow encode(self) -> i32 ![]; }
type First = { value: i32 };
impl First ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }
"#;
    let before = methods(source);
    let after = methods(&source.replace("return self.value;", "return self.value + 1;"));
    let reordered = methods(&source.replace(
        "spec Encoder",
        "flow unrelated() -> i32 ![] { return 0; }\nspec Encoder",
    ));
    assert_eq!(before.len(), 1, "{before:?}");
    assert_eq!(before, after);
    assert_eq!(before, reordered);
}

#[test]
fn applied_nominal_implementation_identity_includes_checked_type_arguments() {
    let methods = all_methods(
        r#"
module library.api;
spec Encoder { flow encode(self) -> i32 ![]; }
type Container<T> = { value: T };
impl Container<i32> ~ Encoder { flow encode(self) -> i32 ![] { return 1; } }
impl Container<string> ~ Encoder { flow encode(self) -> i32 ![] { return 2; } }
"#,
    );
    assert_eq!(methods.len(), 2, "{methods:?}");
    assert_eq!(
        methods[0].implementation().subject(),
        methods[1].implementation().subject()
    );
    assert_ne!(
        methods[0].implementation().header(),
        methods[1].implementation().header()
    );
    assert_ne!(methods[0], methods[1]);
}

#[test]
fn public_type_methods_keep_their_canonical_ids_in_binary_descriptors() {
    let source = r#"
module library.api;
public spec Encoder { flow encode(self) -> i32 ![]; }
public type First = { value: i32 };
public type Second = { value: i32 };
impl First ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }
impl Second ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }
"#;
    let expected = all_methods(source)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(expected.len(), 2);
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(package()),
            package_id: "library".into(),
            package_version: "0.1.0".into(),
            package_edition: "2026".into(),
            compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: "blake3:source".into(),
            manifest_hash: "blake3:manifest".into(),
            dependency_lock_hash: "blake3:lock".into(),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &checked(source),
    )
    .expect("public impl method descriptors");
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("library.etasmeta"),
        &artifact.bytes,
    )
    .unwrap();
    let headers = metadata
        .public_metadata
        .definition_descriptors
        .unwrap()
        .headers
        .unwrap();
    let actual = headers
        .callables
        .iter()
        .filter_map(|header| header.id.as_method().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn applied_alias_method_contracts_publish_with_canonical_method_identity() {
    let source = r#"
module library.api;
public spec Encoder { flow encode(self) -> i32 ![]; flow check(self) -> i32 ![]; }
public spec Pure<I, O> I => O ![];
public spec ReadOnly: trace = -Console.stdout_write;
public type Container<T> = { value: T };
public alias StringBox = Container<string>;
impl StringBox ~ Encoder {
    flow encode(self) -> i32 ![] ~ ReadOnly { return 1; }
    flow check(self) -> i32 ![] ~ Pure<StringBox, i32> { return 2; }
}
"#;
    let artifact = etas_frontend::build_package_metadata_artifact_from_checked(
        etas_frontend::PackageMetadataBuildInput {
            definition_origin: Some(package()),
            package_id: "library".into(),
            package_version: "0.1.0".into(),
            package_edition: "2026".into(),
            compiler_version: etas_frontend::FRONTEND_COMPILER_VERSION.into(),
            source_payload_hash: "blake3:source".into(),
            manifest_hash: "blake3:manifest".into(),
            dependency_lock_hash: "blake3:lock".into(),
            bins: vec![],
            dependencies: vec![],
            tool_bindings: vec![],
        },
        &checked(source),
    )
    .expect("impl method contracts must not require top-level flow export paths");
    let (_, metadata) = etas_package_metadata::package_metadata_from_artifact(
        std::path::Path::new("library.etasmeta"),
        &artifact.bytes,
    )
    .unwrap();
    let public = metadata.public_metadata;
    let implementations = &public.spec_impls[0].methods;
    assert_eq!(public.trace_spec_conformances.len(), 1);
    assert_eq!(public.callable_spec_satisfactions.len(), 1);
    for (target, name) in [
        (&public.trace_spec_conformances[0].item, "encode"),
        (&public.callable_spec_satisfactions[0].item, "check"),
    ] {
        let etas_package_metadata::CallableContractTarget::Method(identity) = target else {
            panic!("impl method must not be published as an ordinary declaration: {target:?}");
        };
        assert_eq!(identity.name(), name);
        assert_eq!(
            identity.implementation().subject().declaration(),
            ["Container"]
        );
        assert_eq!(
            implementations
                .iter()
                .filter(|method| &method.identity == identity)
                .count(),
            1
        );
    }
    assert!(
        public.flows.is_empty(),
        "method bodies are not public top-level flows"
    );
    assert_eq!(public.trace_spec_summaries.len(), 1);
}

#[test]
fn same_nominal_subject_with_different_spec_headers_has_distinct_methods() {
    let methods = all_methods(
        r#"
module library.api;
spec First { flow encode(self) -> i32 ![]; }
spec Second { flow encode(self) -> i32 ![]; }
type Packet = { value: i32 };
impl Packet ~ First { flow encode(self) -> i32 ![] { return self.value; } }
impl Packet ~ Second { flow encode(self) -> i32 ![] { return self.value + 1; } }
"#,
    );
    assert_eq!(methods.len(), 2);
    assert_eq!(
        methods[0].implementation().subject(),
        methods[1].implementation().subject()
    );
    assert_ne!(
        methods[0].implementation().header(),
        methods[1].implementation().header()
    );
    assert_ne!(methods[0], methods[1]);
}

#[test]
fn implementation_order_does_not_become_part_of_method_identity() {
    let prefix = r#"
module library.api;
spec Encoder { flow encode(self) -> i32 ![]; }
type First = { value: i32 };
type Second = { value: i32 };
"#;
    let first = "impl First ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }";
    let second = "impl Second ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }";
    let a = methods(&format!("{prefix}\n{first}\n{second}"));
    let b = methods(&format!("{prefix}\n{second}\n{first}"));
    assert_eq!(a.len(), 2);
    assert_eq!(a, b);
}

#[test]
fn transparent_alias_spelling_does_not_replace_the_nominal_method_subject() {
    let source = r#"
module library.api;
spec Encoder { flow encode(self) -> i32 ![]; }
type First = { value: i32 };
impl First ~ Encoder { flow encode(self) -> i32 ![] { return self.value; } }
"#;
    let direct = methods(source);
    let aliased = methods(&source.replace(
        "impl First ~ Encoder",
        "alias Alternate = First;\nimpl Alternate ~ Encoder",
    ));
    assert_eq!(direct.len(), 1);
    assert_eq!(direct, aliased);
}
