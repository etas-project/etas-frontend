use std::{collections::BTreeMap, sync::Arc};

use etas_code::{CodeFingerprint, PackageKey, report::CheckStatus, syntax::CodeSyntaxOperations};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment},
        draft::DefaultBodyOverlay,
        identity::DefinitionOrigins,
        service::{CandidateCompileError, CheckedCandidate, NativeCodeSyntaxOperations},
    },
};

fn frozen(source: &str) -> Arc<FrozenDefinitionEnvironment> {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.candidate",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let expression = *checked.types.reflections.keys().next().unwrap();
    DefinitionCatalog::build(checked, &origins, Default::default())
        .unwrap()
        .freeze_agent(expression)
        .unwrap()
}

fn overlay(frozen: Arc<FrozenDefinitionEnvironment>, body: &str) -> DefaultBodyOverlay {
    let syntax = NativeCodeSyntaxOperations::default();
    DefaultBodyOverlay::prepare(frozen, syntax.parse_body(body).unwrap(), &syntax).unwrap()
}

const PURE: &str = "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }";

#[test]
fn compile_without_a_prior_report_emits_a_native_checked_candidate() {
    let frozen = frozen(PURE);
    let replacement = overlay(frozen.clone(), "return 2;");
    let expected_snapshot = replacement.fingerprint();
    let expected_revision = replacement.revision().clone();
    let candidate = FrontendSession::new()
        .compile_body_overlay(replacement)
        .unwrap();
    assert_eq!(candidate.snapshot_fingerprint(), expected_snapshot);
    assert_eq!(*candidate.revision(), expected_revision);
    assert_eq!(candidate.identity(), frozen.identity());
    assert_eq!(candidate.program().entry, Some(candidate.entry()));
    assert_eq!(
        candidate.revision().inputs().contract,
        frozen.contract().fingerprint()
    );
    assert!(Arc::ptr_eq(candidate.frozen(), &frozen));
    assert!(!Arc::ptr_eq(candidate.program(), frozen.checked()));
    assert_eq!(
        candidate
            .revision()
            .inputs()
            .definition
            .as_method()
            .unwrap()
            .implementation()
            .subject(),
        candidate.identity()
    );
}

#[test]
fn successful_native_emission_is_a_real_pass_artifact() {
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay(frozen(PURE), "return 2;"))
        .unwrap();
    let emit = analysis
        .pipeline_records()
        .iter()
        .find(|record| record.pass == "EmitCheckedCandidatePass")
        .unwrap();
    assert!(
        emit.produced
            .iter_refs()
            .any(|artifact| artifact.key.namespace == "code"
                && artifact.key.name == "candidate_emission")
    );
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        CheckStatus::Accepted
    );
    analysis.into_checked_candidate(Default::default()).unwrap();
}

#[test]
fn a_type_valid_but_incompatible_body_cannot_emit_a_checked_candidate() {
    let frozen = frozen(
        "module app; import std.io.println; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let analysis = FrontendSession::new()
        .analyze_body_overlay(overlay(frozen, "println(\"new request\")?; return 2;"))
        .unwrap();
    assert!(analysis.output().checked.is_some());
    let emit = analysis
        .pipeline_records()
        .iter()
        .find(|record| record.pass == "EmitCheckedCandidatePass")
        .unwrap();
    assert!(
        emit.produced
            .iter_refs()
            .any(|artifact| artifact.key.name == "candidate_emission")
    );
    match analysis.into_checked_candidate(Default::default()) {
        Err(CandidateCompileError::Rejected(report)) => {
            assert_eq!(report.status, CheckStatus::Rejected);
            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "code::AdditionalRequestedActions")
            );
        }
        Err(error) => panic!("expected ordinary rejection, got {error}"),
        Ok(_) => panic!("incompatible candidate was emitted"),
    }
}

#[test]
fn source_errors_and_unproved_contracts_are_both_rejections_not_backend_permits() {
    for (source, replacement, code) in [
        (PURE, "return missing;", "name::UnresolvedName"),
        (
            "module app; import std.io.println; agent Worker() -> Result<unit, IOError> { return println(\"old\")?; } flow inspect() -> unit { let d = ^Worker; return; }",
            "return println(\"new\")?;",
            "code::MissingOutcomeSensitiveSummary",
        ),
    ] {
        match FrontendSession::new().compile_body_overlay(overlay(frozen(source), replacement)) {
            Err(CandidateCompileError::Rejected(report)) => assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == code)
            ),
            Err(error) => panic!("expected rejected report, got {error}"),
            Ok(_) => panic!("unproved candidate was emitted"),
        }
    }
}

#[test]
fn a_prior_accepted_report_does_not_authorize_a_different_body() {
    let frozen = frozen(
        "module app; agent Worker() -> bool { return true; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let mut session = FrontendSession::new();
    let checked = session
        .analyze_body_overlay(overlay(frozen.clone(), "return true;"))
        .unwrap();
    let report = checked.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Accepted);
    match session.compile_body_overlay(overlay(frozen, "return false;")) {
        Err(CandidateCompileError::Rejected(rejected)) => {
            assert_ne!(rejected.snapshot_fingerprint, report.snapshot_fingerprint);
            assert!(
                rejected
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "code::OutcomeTraceNotContained")
            );
        }
        Err(error) => panic!("expected rejection, got {error}"),
        Ok(_) => panic!("stale accepted report authorized a different revision"),
    }
}

#[test]
fn successive_compilations_keep_the_original_contract_and_do_not_mutate_the_base() {
    let frozen = frozen(PURE);
    let original = frozen.checked().sources.sources[0].text().to_owned();
    let mut session = FrontendSession::new();
    let first = session
        .compile_body_overlay(overlay(frozen.clone(), "return 2;"))
        .unwrap();
    let second = session
        .compile_body_overlay(overlay(frozen.clone(), "return 3;"))
        .unwrap();
    assert_eq!(first.identity(), second.identity());
    assert_ne!(first.revision().key(), second.revision().key());
    assert_eq!(
        first.revision().inputs().contract,
        second.revision().inputs().contract
    );
    assert!(Arc::ptr_eq(first.frozen(), second.frozen()));
    assert!(!Arc::ptr_eq(first.program(), second.program()));
    assert_eq!(frozen.checked().sources.sources[0].text(), original);
}

#[test]
fn native_worker_output_contains_only_sendable_immutable_compiler_data() {
    fn require_send_sync<T: Send + Sync>() {}
    require_send_sync::<CheckedCandidate>();
}

#[test]
fn backend_support_uses_the_selected_agent_entry_not_a_flow_named_main() {
    let frozen = frozen(
        "module app; import std.io.println; agent Worker() -> i32 { return 1; } flow main() -> unit ![Error<IOError>] { println(\"unrelated\"); return; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let candidate = FrontendSession::new()
        .compile_body_overlay(overlay(frozen, "return 2;"))
        .unwrap();
    let support = &candidate.program().effects.interpreter_support;
    assert_eq!(
        support.entry,
        Some(etas_effects::InterpreterSupport::LocalOnly)
    );
    assert_eq!(
        support.items.get(&candidate.entry()),
        Some(&etas_effects::InterpreterSupport::LocalOnly)
    );
}

#[test]
fn never_is_not_evidence_that_raised_errors_and_other_termination_are_interchangeable() {
    for (source, replacement) in [
        (
            "module app; agent Worker() -> i32 ![Error<string>] { return abort(\"old\"); } flow inspect() -> unit { let d = ^Worker; return; }",
            "return perform Error<string>.raise(\"different exit\");",
        ),
        (
            "module app; flow stop() -> never ![Error<string>] { return perform Error<string>.raise(\"old\"); } agent Worker() -> i32 ![Error<string>] { return stop(); } flow inspect() -> unit { let d = ^Worker; return; }",
            "return abort(\"different exit\");",
        ),
    ] {
        match FrontendSession::new().compile_body_overlay(overlay(frozen(source), replacement)) {
            Err(CandidateCompileError::Rejected(report)) => assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "code::MissingOutcomeSensitiveSummary")
            ),
            Err(error) => panic!("expected an explicit unproved-contract rejection, got {error}"),
            Ok(_) => panic!(
                "never erased the distinction between raising an Error and another termination"
            ),
        }
    }
}
