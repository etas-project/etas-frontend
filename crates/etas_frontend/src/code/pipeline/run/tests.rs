use super::*;
use crate::code::{
    contract::CandidateContractAnalysis,
    definition::DefinitionCatalog,
    identity::DefinitionOrigins,
    service::{CheckReportError, NativeCodeSyntaxOperations},
};
use crate::{EntryPolicy, Frontend, FrontendSession, SourceInput};
use etas_code::{PackageKey, syntax::CodeSyntaxOperations};
use std::collections::BTreeMap;

fn analyzed(body: &str) -> CandidateBodyAnalysis {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    ));
    input.options.entry_policy = EntryPolicy::Optional;
    let checked = Frontend.check_project(input).checked.unwrap();
    let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
        "etas.test.report",
        b"app",
    ));
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let expression = *checked.types.reflections.keys().next().unwrap();
    let frozen = DefinitionCatalog::build(checked, &origins, Default::default())
        .unwrap()
        .freeze_agent(expression)
        .unwrap();
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body(body).unwrap(), &syntax).unwrap();
    FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
}

#[test]
fn checked_source_without_contract_artifact_is_a_structural_failure() {
    let mut analysis = analyzed("return 2;");
    analysis.contract = None;
    assert!(matches!(
        analysis.check_report(Default::default()),
        Err(CheckReportError::IncompleteAnalysis(_))
    ));
}

#[test]
fn reports_and_emitted_candidates_share_the_pipeline_descriptor_artifact() {
    let analysis = analyzed("return 2;");
    let first = analysis
        .definition_catalog(Default::default())
        .unwrap()
        .unwrap();
    let second = analysis
        .definition_catalog(Default::default())
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(
        &first,
        &analysis.emitted.as_ref().unwrap().descriptors
    ));
    assert!(Arc::ptr_eq(
        &first,
        &analysis
            .emitted
            .as_ref()
            .unwrap()
            .definition_catalog(Default::default())
            .unwrap()
    ));
    let report = analysis.check_report(Default::default()).unwrap();
    assert!(std::ptr::eq(
        report.descriptors.data(),
        first.descriptors().data()
    ));
    let records = analysis.pipeline_records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.pass == "BuildDefinitionDescriptorsPass")
            .count(),
        1
    );
}

#[test]
fn missing_or_foreign_descriptor_artifacts_do_not_trigger_an_implicit_rebuild() {
    let mut analysis = analyzed("return 2;");
    let mut other = analyzed("return 3;");
    analysis.descriptors = None;
    assert!(analysis.check_report(Default::default()).is_err());
    assert!(analysis.definition_catalog(Default::default()).is_err());
    analysis.descriptors = other.descriptors.take();
    assert!(analysis.check_report(Default::default()).is_err());
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

#[test]
fn an_existing_emission_cannot_bypass_a_stricter_descriptor_limit() {
    let analysis = analyzed("return 2;");
    let limits = etas_code::descriptor::DescriptorLimits {
        max_nodes: 0,
        ..Default::default()
    };
    assert!(matches!(
        analysis.into_checked_candidate(limits),
        Err(crate::code::service::CandidateCompileError::Report(
            CheckReportError::Projection(_)
        ))
    ));
}

#[test]
fn omitting_or_duplicating_a_contract_component_cannot_produce_an_accepted_report() {
    let mut analysis = analyzed("return 2;");
    let Some(CandidateContractAnalysis::Analyzed { checks, .. }) = &mut analysis.contract else {
        panic!("analysis")
    };
    let removed = checks.pop().unwrap();
    assert!(matches!(
        analysis.check_report(Default::default()),
        Err(CheckReportError::InvalidContract(_))
    ));
    let Some(CandidateContractAnalysis::Analyzed { checks, .. }) = &mut analysis.contract else {
        panic!("analysis")
    };
    checks.push(checks[0].clone());
    assert!(matches!(
        analysis.check_report(Default::default()),
        Err(CheckReportError::InvalidContract(_))
    ));
    let Some(CandidateContractAnalysis::Analyzed { checks, .. }) = &mut analysis.contract else {
        panic!("analysis")
    };
    checks.pop();
    checks.push(removed);
    assert_eq!(
        analysis.check_report(Default::default()).unwrap().status,
        etas_code::report::CheckStatus::Accepted
    );
}

#[test]
fn contract_proof_from_another_snapshot_is_not_reused() {
    let mut analysis = analyzed("return 2;");
    let Some(CandidateContractAnalysis::Analyzed { snapshot, .. }) = &mut analysis.contract else {
        panic!("analysis")
    };
    *snapshot = CodeFingerprint::of_bytes("etas.test.wrong-snapshot", b"different");
    assert!(matches!(
        analysis.check_report(Default::default()),
        Err(CheckReportError::InvalidContract(_))
    ));
}

#[test]
fn missing_diagnostic_source_is_not_replaced_with_the_project_or_call_site() {
    let mut analysis = analyzed("return missing;");
    analysis.output.diagnostics[0].primary.span.source = etas_core::SourceId(u32::MAX);
    assert!(matches!(
        analysis.check_report(Default::default()),
        Err(CheckReportError::MissingSource(_))
    ));
}

#[test]
fn diagnostic_byte_boundaries_are_checked_before_a_report_is_returned() {
    let mut analysis = analyzed("let text = \"中文\"; return missing;");
    analysis.output.diagnostics[0].primary.span.range =
        etas_core::TextRange::new(etas_core::TextSize::new(13), etas_core::TextSize::new(14));
    assert!(matches!(
        analysis.check_report(Default::default()),
        Err(CheckReportError::InvalidSpan(_))
    ));
}

#[test]
fn an_accepted_report_cannot_replace_missing_native_emission() {
    let mut analysis = analyzed("return 2;");
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, etas_code::report::CheckStatus::Accepted);
    analysis.emitted = None;
    assert!(matches!(
        analysis.into_checked_candidate(Default::default()),
        Err(crate::code::service::CandidateCompileError::Report(
            CheckReportError::IncompleteAnalysis(_)
        ))
    ));
}

#[test]
fn native_emission_cannot_be_substituted_from_a_different_candidate() {
    let mut first = analyzed("return 2;");
    let mut second = analyzed("return 3;");
    first.emitted = second.emitted.take();
    assert!(matches!(
        first.into_checked_candidate(Default::default()),
        Err(crate::code::service::CandidateCompileError::Report(
            CheckReportError::IncompleteAnalysis(_)
        ))
    ));
}
