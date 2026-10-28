use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, PackageKey,
    definition::{
        AgentSourceBundle, AnalysisUnavailableReason, CodeDefinitionOperations, SourceBundleError,
        SourceBundleLimits,
    },
    syntax::CodeSyntaxOperations,
};
use etas_frontend::{
    Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, NativeDefinitionError, NativeDefinitionStore},
        identity::DefinitionOrigins,
        service::NativeCodeSyntaxOperations,
    },
};

const SOURCE: &str = "module app; agent Worker() -> i32 { return helper(); } flow helper() -> i32 { return 1; } flow main() -> unit { let definition = ^Worker; return; }";

fn prepare(source: &str, retained: bool) -> (Arc<NativeDefinitionStore>, etas_hir::HirExprId) {
    prepare_with_id(source, retained, None)
}

fn prepare_with_id(
    source: &str,
    retained: bool,
    id: Option<etas_core::SourceId>,
) -> (Arc<NativeDefinitionStore>, etas_hir::HirExprId) {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    if let Some(id) = id {
        input.sources[0].id = id;
    }
    if retained {
        input
            .options
            .reflection
            .retain_bodies
            .push("app.Worker".into());
    }
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let expression = *checked.types.reflections.keys().next().unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.source-restoration.v1",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: etas_frontend::code::identity::standard_package_key(
            &checked.std_registry,
        )
        .unwrap(),
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
    (
        Arc::new(NativeDefinitionStore::new(
            catalog,
            Arc::new(NativeCodeSyntaxOperations::default()),
            Default::default(),
        )),
        expression,
    )
}

#[test]
fn source_bundle_restores_exact_revision_as_unchecked_data_in_a_fresh_backend() {
    let (store, expression) = prepare(SOURCE, true);
    let original = store.reflect_agent(expression).unwrap();
    let first = store
        .with_body(
            &original,
            store
                .syntax_operations()
                .parse_body("if helper() == 1 { return 2; } return 3;")
                .unwrap(),
        )
        .unwrap();
    let second = store
        .with_body(
            &first,
            store
                .syntax_operations()
                .parse_body("if helper() == 1 { return 3; } return 4;")
                .unwrap(),
        )
        .unwrap();
    let candidate = FrontendSession::new()
        .compile_body_overlay(store.body_overlay(&second).unwrap())
        .unwrap();
    let checked = store
        .reflect_checked(&second, &candidate, Default::default())
        .unwrap();
    assert!(store.resolve(&checked).unwrap().current_analysis().is_ok());
    let saved = store.save_source(&checked, Default::default()).unwrap();
    assert_eq!(saved.revision.parent, Some(first.revision().key()));
    let bytes = serde_json::to_vec(&saved).unwrap();
    drop((store, original, first, second, checked, candidate));

    let saved: AgentSourceBundle = serde_json::from_slice(&bytes).unwrap();
    let (store, expression) = prepare_with_id(SOURCE, true, Some(etas_core::SourceId(73)));
    let target = store.reflect_agent(expression).unwrap();
    let restored = store
        .restore_source_draft(&target, &saved, Default::default())
        .unwrap();
    assert_eq!(restored.revision().inputs(), &saved.revision);
    assert!(!restored.is_same_object(&target));
    let view = store.resolve(&restored).unwrap();
    assert_eq!(
        view.current_analysis(),
        Err(AnalysisUnavailableReason::NotAnalyzed)
    );
    assert!(view.checked_implementation().is_none());
    let candidate = FrontendSession::new()
        .compile_body_overlay(store.body_overlay(&restored).unwrap())
        .unwrap();
    assert_eq!(candidate.revision(), restored.revision());
    let accepted = store
        .reflect_checked(&restored, &candidate, Default::default())
        .unwrap();
    assert!(store.resolve(&accepted).unwrap().current_analysis().is_ok());
}

#[test]
fn restored_source_does_not_replay_old_analysis_as_compile_evidence() {
    let (store, expression) = prepare(SOURCE, true);
    let original = store.reflect_agent(expression).unwrap();
    for (body, expected) in [
        (
            "return helper() + 3;",
            "code::MissingOutcomeSensitiveSummary",
        ),
        ("return false;", "type::TypeMismatch"),
    ] {
        let edited = store
            .with_body(
                &original,
                store.syntax_operations().parse_body(body).unwrap(),
            )
            .unwrap();
        let saved = store.save_source(&edited, Default::default()).unwrap();
        let (fresh, expression) = prepare(SOURCE, true);
        let target = fresh.reflect_agent(expression).unwrap();
        let restored = fresh
            .restore_source_draft(&target, &saved, Default::default())
            .unwrap();
        let analysis = FrontendSession::new()
            .analyze_body_overlay(fresh.body_overlay(&restored).unwrap())
            .unwrap();
        let report = analysis.check_report(Default::default()).unwrap();
        assert_eq!(report.status, etas_code::report::CheckStatus::Rejected);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == expected),
            "{:?}",
            report.diagnostics
        );
    }
}

#[test]
fn restored_source_rejects_changed_contract_environment_dependencies_and_configuration() {
    let (store, expression) = prepare(SOURCE, true);
    let original = store.reflect_agent(expression).unwrap();
    let saved = store.save_source(&original, Default::default()).unwrap();
    for (source, failure) in [
        (
            SOURCE.replace("-> i32 { return helper(); }", "-> bool { return true; }"),
            SourceBundleError::ContractMismatch,
        ),
        (
            SOURCE.replace("return 1;", "return 9;"),
            SourceBundleError::EnvironmentMismatch,
        ),
    ] {
        let (fresh, expression) = prepare(&source, true);
        let target = fresh.reflect_agent(expression).unwrap();
        assert!(
            matches!(fresh.restore_source_draft(&target, &saved, Default::default()), Err(NativeDefinitionError::SourceBundle(error)) if error == failure)
        );
    }
    let different = CodeFingerprint::of_bytes("etas.test.source-restoration.changed", b"changed");
    for (change, failure) in [
        (0, SourceBundleError::ConfigurationMismatch),
        (1, SourceBundleError::DependencyMismatch),
    ] {
        let mut altered = saved.clone();
        if change == 0 {
            altered.revision.compilation = different;
            altered.revision_key = etas_code::RevisionKey::from_inputs(&altered.revision);
        } else {
            altered.dependencies.standard_package =
                PackageKey::from_canonical_fingerprint(different);
        }
        assert!(
            matches!(store.restore_source_draft(&original, &altered, Default::default()), Err(NativeDefinitionError::SourceBundle(error)) if error == failure)
        );
    }
}

#[test]
fn saving_source_respects_body_visibility_and_restoration_is_bounded() {
    let (hidden, expression) = prepare(SOURCE, false);
    let original = hidden.reflect_agent(expression).unwrap();
    assert!(matches!(
        hidden.save_source(&original, Default::default()),
        Err(NativeDefinitionError::MissingBodySource)
    ));
    let (store, expression) = prepare(SOURCE, true);
    let original = store.reflect_agent(expression).unwrap();
    let saved = store.save_source(&original, Default::default()).unwrap();
    for limits in [
        SourceBundleLimits {
            max_source_bytes: 0,
            ..Default::default()
        },
        SourceBundleLimits {
            max_environment_entries: 0,
            ..Default::default()
        },
        SourceBundleLimits {
            max_environment_bytes: 0,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            store.restore_source_draft(&original, &saved, limits),
            Err(NativeDefinitionError::SourceBundle(
                SourceBundleError::ResourceLimit { .. }
            ))
        ));
    }
    let mut corrupt = saved;
    corrupt.source.text.push_str("return 0;");
    assert!(matches!(
        store.restore_source_draft(&original, &corrupt, Default::default()),
        Err(NativeDefinitionError::SourceBundle(
            SourceBundleError::InvalidSource(_)
        ))
    ));
}

#[test]
fn restoration_preserves_block_expression_and_trailing_handler_source_forms() {
    use etas_code::syntax::RetainedBodyForm as F;
    for (method, form) in [
        ("{ return helper(); }", F::BlockContents),
        ("= helper();", F::Expression),
        (
            "{ return helper(); } with { Error<i32>.raise(_) => { finish 0; } }",
            F::HandledBlock,
        ),
        (
            "= helper(); with { Error<i32>.raise(_) => { finish 0; } }",
            F::HandledExpression,
        ),
    ] {
        let source = format!(
            "module app; agent Worker; impl agent Worker {{ public flow run() -> i32 {method} }} flow helper() -> i32 {{ return 1; }} flow main() -> unit {{ let definition = ^Worker; return; }}"
        );
        let (store, expression) = prepare(&source, true);
        let target = store.reflect_agent(expression).unwrap();
        let saved = store.save_source(&target, Default::default()).unwrap();
        assert_eq!(saved.source.form, form);
        drop((store, target));
        let (fresh, expression) = prepare(&source, true);
        let target = fresh.reflect_agent(expression).unwrap();
        let restored = fresh
            .restore_source_draft(&target, &saved, Default::default())
            .unwrap();
        let saved_again = fresh.save_source(&restored, Default::default()).unwrap();
        assert_eq!(saved_again.source.form, form);
        assert_eq!(saved_again.source.text, saved.source.text);
        assert_eq!(saved_again.revision, saved.revision);
        FrontendSession::new()
            .compile_body_overlay(fresh.body_overlay(&restored).unwrap())
            .unwrap();
    }
}

#[test]
fn cancellation_during_restore_does_not_publish_or_reuse_an_unchecked_draft() {
    struct Cancel {
        checkpoints: std::sync::atomic::AtomicUsize,
    }
    impl etas_utils::PipelineExecutionControl for Cancel {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self
                .checkpoints
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                >= 2
            {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel source restoration".into(),
                })
            } else {
                Ok(())
            }
        }
    }
    let (store, expression) = prepare(SOURCE, true);
    let target = store.reflect_agent(expression).unwrap();
    let saved = store.save_source(&target, Default::default()).unwrap();
    let cached = store
        .restore_source_draft(&target, &saved, Default::default())
        .unwrap();
    let before = store.retained_bytes().unwrap();
    let control = Cancel {
        checkpoints: Default::default(),
    };
    let result =
        store.restore_source_draft_controlled(&target, &saved, Default::default(), &control);
    assert!(
        matches!(result, Err(NativeDefinitionError::Operation(etas_code::definition::DefinitionOperationError::Syntax(etas_code::syntax::SyntaxOperationError::Interrupted(error)))) if error.kind == etas_utils::PipelineInterruptionKind::Cancelled)
    );
    assert_eq!(store.retained_bytes().unwrap(), before);
    assert!(
        store
            .resolve(&cached)
            .unwrap()
            .checked_implementation()
            .is_none()
    );
}

#[test]
fn self_consistent_malformed_source_is_reparsed_without_publishing_a_definition() {
    let (store, expression) = prepare(SOURCE, true);
    let original = store.reflect_agent(expression).unwrap();
    let saved = store.save_source(&original, Default::default()).unwrap();
    let before = store.retained_bytes().unwrap();
    for text in [
        "let = ;".to_owned(),
        format!("{}1{}", "(".repeat(8192), ")".repeat(8192)),
    ] {
        let mut malformed = saved.clone();
        malformed.source.text = text;
        malformed.source.source_start = 0;
        malformed.source.source_length = malformed.source.text.len() as u64;
        malformed.source.source_unit =
            CodeFingerprint::of_bytes("etas.code.source-unit.v1", malformed.source.text.as_bytes());
        malformed.source.content = CodeFingerprint::of_bytes(
            "etas.code.body-content.v1",
            malformed.source.text.as_bytes(),
        );
        malformed.revision.content = malformed.source.content;
        malformed.revision_key = etas_code::RevisionKey::from_inputs(&malformed.revision);
        malformed.validate(Default::default()).unwrap();
        assert!(
            store
                .restore_source_draft(&original, &malformed, Default::default())
                .is_err()
        );
        assert_eq!(store.retained_bytes().unwrap(), before);
    }
}

#[test]
fn missing_revision_lineage_is_rejected_instead_of_creating_a_new_revision() {
    let (store, expression) = prepare(SOURCE, true);
    let original = store.reflect_agent(expression).unwrap();
    let draft = store
        .with_body(
            &original,
            store
                .syntax_operations()
                .parse_body("return helper();")
                .unwrap(),
        )
        .unwrap();
    let mut source = store.save_source(&draft, Default::default()).unwrap();
    assert!(source.revision.parent.is_some());
    source.revision.parent = None;
    assert!(matches!(
        store.restore_source_draft(&original, &source, Default::default()),
        Err(NativeDefinitionError::SourceBundle(
            SourceBundleError::RevisionMismatch
        ))
    ));
}
