use super::*;
use std::collections::BTreeMap;

use crate::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::FrozenDefinitionEnvironment, identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};
use etas_code::{CodeFingerprint, PackageKey, syntax::CodeSyntaxOperations};

fn prepared() -> (DefaultBodyOverlay, CheckedProgram) {
    let source = "module app; agent Worker() -> i32 { return 1; } impl agent Worker { flow echo(value: i32) -> i32 { return value; } } flow inspect() -> unit { let definition = ^Worker; return; }";
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.preservation",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let reflection = *checked.types.reflections.keys().next().unwrap();
    let frozen: Arc<FrozenDefinitionEnvironment> =
        DefinitionCatalog::build(checked, &origins, Default::default())
            .unwrap()
            .freeze_agent(reflection)
            .unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body("return 2;").unwrap(), &syntax)
            .unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay.clone())
        .unwrap();
    (overlay, candidate.program().clone())
}

#[test]
fn only_an_unedited_native_body_has_a_preservation_proof() {
    let (overlay, candidate) = prepared();
    let proof = BodyPreservation::verify(&overlay, &candidate).unwrap();
    let origins = overlay.frozen().origins_for(&candidate).unwrap();
    let catalog =
        DefinitionCatalog::build(candidate.clone(), &origins, Default::default()).unwrap();
    for (id, _) in overlay.frozen().contract().callables() {
        assert_eq!(
            proof.preserves(id, &catalog).unwrap(),
            overlay.frozen().contract().header().default_entry.as_ref() != Some(id)
        );
    }
}

#[test]
fn preservation_rejects_changed_environment_or_missing_native_overlay() {
    let (overlay, candidate) = prepared();
    let mut changed = (*candidate).clone();
    changed.compiler_version.push_str("-changed");
    assert!(BodyPreservation::verify(&overlay, &Arc::new(changed)).is_err());
    let mut changed = (*candidate).clone();
    changed.definition_sources = overlay.frozen().checked().definition_sources.clone();
    assert!(BodyPreservation::verify(&overlay, &Arc::new(changed)).is_err());
}

#[test]
fn a_matching_fingerprint_is_not_native_transform_evidence() {
    let (overlay, candidate) = prepared();
    let syntax = NativeCodeSyntaxOperations::default();
    let reconstructed = DefaultBodyOverlay::prepare(
        overlay.frozen().clone(),
        syntax.parse_body("return 2;").unwrap(),
        &syntax,
    )
    .unwrap();
    assert_eq!(overlay.fingerprint(), reconstructed.fingerprint());
    assert!(BodyPreservation::verify(&reconstructed, &candidate).is_err());
}
