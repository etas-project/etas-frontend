use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, PackageKey,
    descriptor::DescriptorLimits,
    diagnostic::{DescriptorDetail, DiagnosticSeverity},
    report::CheckStatus,
    syntax::{CodeSyntaxOperations, SyntaxIdentity},
};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment},
        draft::DefaultBodyOverlay,
        identity::DefinitionOrigins,
        service::{CandidateBodyAnalysis, NativeCodeSyntaxOperations},
    },
};

fn freeze(source: &str) -> Arc<FrozenDefinitionEnvironment> {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let package = |bytes| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.report-package",
            bytes,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package(b"app"))
            .unwrap(),
        standard_package: package(b"std"),
        external_definitions: BTreeMap::new(),
    };
    let expression = *checked.types.reflections.keys().next().unwrap();
    DefinitionCatalog::build(checked, &origins, DescriptorLimits::default())
        .unwrap()
        .freeze_agent(expression)
        .unwrap()
}

fn analyze(
    frozen: Arc<FrozenDefinitionEnvironment>,
    body: &str,
) -> (CandidateBodyAnalysis, SyntaxIdentity) {
    let syntax = NativeCodeSyntaxOperations::default();
    let block = syntax.parse_body(body).unwrap();
    let identity = block.identity();
    let overlay = DefaultBodyOverlay::prepare(frozen, block, &syntax).unwrap();
    (
        FrontendSession::new()
            .analyze_body_overlay(overlay)
            .unwrap(),
        identity,
    )
}

const PURE: &str = "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }";

#[test]
fn accepted_report_contains_only_real_candidate_analysis_and_snapshot_coordinates() {
    let frozen = freeze(PURE);
    let environment = frozen.fingerprint();
    let contract = frozen.contract().fingerprint();
    let (analysis, syntax) = analyze(frozen, "return 2;");
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Accepted);
    assert!(report.diagnostics.is_empty());
    assert!(report.runtime_checks.is_empty());
    assert_eq!(report.snapshot_fingerprint, analysis.fingerprint());
    assert_eq!(report.revision.inputs().content, syntax.content());
    assert_eq!(report.revision.inputs().environment, environment);
    assert_eq!(report.revision.inputs().contract, contract);
    assert_eq!(report.dependencies.environment, environment);
    assert_eq!(
        report.dependencies.lexical_owner,
        *analysis.frozen().identity()
    );
    assert_eq!(report.dependencies.modules.len(), 1);
    assert_eq!(report.dependencies.modules[0].module, ["app"]);
    assert!(report.effects.declared_bound.is_none());
    for row in [
        report.effects.escaping,
        report.effects.requested,
        report.effects.default_handled,
    ] {
        let row = report.descriptors.effect_row(row.unwrap()).unwrap();
        assert!(row.effects.is_empty());
        assert!(row.tail.is_none());
    }
    assert!(report.determinism.is_some());
    assert!(report.trace.is_some());
}

#[test]
fn type_valid_source_is_rejected_when_it_adds_a_handled_request() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let body = "println(\"new action\")?; return 2;";
    let (analysis, syntax) = analyze(frozen, body);
    assert!(analysis.output().checked.is_some());
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "code::AdditionalRequestedActions")
        .unwrap();
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    let location = diagnostic.location.as_ref().unwrap();
    assert_eq!(location.source, syntax.source());
    assert_eq!(location.revision, syntax.content());
    assert_eq!((location.byte_start, location.byte_end), (0, body.len()));
    let detail = diagnostic.details.as_ref().unwrap();
    let Some(DescriptorDetail::EffectRow { graph, row }) = &detail.expected else {
        panic!("expected row")
    };
    assert!(graph.effect_row(*row).unwrap().effects.is_empty());
    let Some(DescriptorDetail::EffectRow { graph, row }) = &detail.actual else {
        panic!("actual row")
    };
    assert!(!graph.effect_row(*row).unwrap().effects.is_empty());
    assert_eq!(
        detail.constraint.as_ref().unwrap().kind,
        "requested_actions"
    );
    assert_eq!(
        detail.analysis_path[0].callable,
        report.revision.inputs().definition
    );
}

#[test]
fn outcome_rejection_keeps_both_trace_descriptors_and_the_abstract_witness() {
    let frozen = freeze(
        "module app; agent Worker() -> bool { return true; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let (analysis, _) = analyze(frozen, "return false;");
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "code::OutcomeTraceNotContained")
        .unwrap();
    let details = diagnostic.details.as_ref().unwrap();
    assert!(matches!(
        details.expected,
        Some(DescriptorDetail::Trace { .. })
    ));
    assert!(matches!(
        details.actual,
        Some(DescriptorDetail::Trace { .. })
    ));
    assert!(details.trace_witness.is_some());
}

#[test]
fn unproved_behavior_is_a_rejected_report_not_a_service_error_or_guarded_success() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> Result<unit, IOError> { return println(\"old\")?; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let (analysis, _) = analyze(frozen, "return println(\"new\")?;");
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status.as_str(), "rejected");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::MissingOutcomeSensitiveSummary")
    );
    assert!(report.runtime_checks.is_empty());
    assert!(report.effects.requested.is_some());
}

#[test]
fn rejected_source_keeps_real_utf8_byte_offsets_and_marks_inferred_behavior_unavailable() {
    let (analysis, syntax) = analyze(freeze(PURE), "let text = \"中文\"; return missing;");
    assert!(analysis.output().checked.is_none());
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(report.effects.escaping.is_none());
    assert!(report.effects.requested.is_none());
    assert!(report.effects.default_handled.is_none());
    assert!(report.determinism.is_none());
    assert!(report.trace.is_none());
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "name::UnresolvedName")
        .unwrap();
    let location = diagnostic.location.as_ref().unwrap();
    assert_eq!(location.source, syntax.source());
    assert_eq!(location.revision, syntax.content());
    assert_eq!(
        &"let text = \"中文\"; return missing;"[location.byte_start..location.byte_end],
        "missing"
    );
}

#[test]
fn explicit_empty_declaration_survives_source_rejection_without_stale_inferred_facts() {
    let frozen = freeze(
        "module app; agent Worker() -> i32 ![] { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let (analysis, _) = analyze(frozen, "return missing;");
    let report = analysis.check_report(Default::default()).unwrap();
    assert!(
        report
            .descriptors
            .effect_row(report.effects.declared_bound.unwrap())
            .unwrap()
            .effects
            .is_empty()
    );
    assert!(report.effects.escaping.is_none());
}

#[test]
fn replacing_a_returned_callback_cannot_be_accepted_using_only_the_root_action_row() {
    let frozen = freeze(
        "module app; import std.io.println; alias Callback = () -> unit ![]; agent Worker() -> Callback { return () => { return; }; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let (analysis, _) = analyze(frozen, "return () => { println(\"latent\")?; return; };");
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::MissingLatentBehaviorSummary")
    );
    assert!(
        report
            .descriptors
            .effect_row(report.effects.requested.unwrap())
            .unwrap()
            .effects
            .is_empty()
    );
}

#[test]
fn different_bodies_have_different_revision_and_snapshot_fingerprints() {
    let frozen = freeze(PURE);
    let (first, _) = analyze(frozen.clone(), "return 2;");
    let (second, _) = analyze(frozen, "return 3;");
    let first = first.check_report(Default::default()).unwrap();
    let second = second.check_report(Default::default()).unwrap();
    assert_ne!(first.revision.key(), second.revision.key());
    assert_ne!(first.snapshot_fingerprint, second.snapshot_fingerprint);
    assert_eq!(first.dependencies, second.dependencies);
    assert_eq!(
        first.revision.inputs().contract,
        second.revision.inputs().contract
    );
}

#[test]
fn accepted_reports_preserve_compiler_warnings() {
    let frozen = freeze(
        "module app; agent Worker(flag: bool) -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    );
    let (analysis, _) = analyze(
        frozen,
        "match flag { true => { return 1; }, false => { return 2; }, _ => { return 3; }, }",
    );
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Accepted,
        "{:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "type::RedundantMatchArm"
                && diagnostic.severity == DiagnosticSeverity::Warning)
    );
}
