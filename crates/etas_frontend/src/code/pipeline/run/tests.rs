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
    analyzed_with_source_key(body, None)
}

fn analyzed_with_source_key(
    body: &str,
    key: Option<etas_code::StableSourceKey>,
) -> CandidateBodyAnalysis {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
    ));
    if let Some(key) = key {
        input
            .environment
            .stable_source_keys
            .push(crate::ProjectStableSourceKeyInput {
                source: input.sources[0].id,
                key,
            });
    }
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
fn candidate_generated_body_keeps_preanalysis_source_identity() {
    use etas_code::{SourceRevisionKey, StableSourceKey, StableSourceOrigin};
    let parent = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([74; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let first = analyzed_with_source_key("return 2;", Some(parent));
    let second = analyzed_with_source_key("return 3;", Some(parent));
    let generated = |analysis: &CandidateBodyAnalysis| {
        let checked = analysis.output.checked.as_ref().unwrap();
        let key = checked.sources.stable_source_key(analysis.source).unwrap();
        let source = checked
            .sources
            .sources
            .iter()
            .find(|source| source.id == analysis.source)
            .unwrap();
        (key, SourceRevisionKey::new(key, source.text.as_bytes()))
    };
    let first = generated(&first);
    let second = generated(&second);
    assert_eq!(first.0, second.0);
    assert_ne!(first.1, second.1);
    assert_ne!(first.0, parent);
}

#[test]
fn candidate_dependency_fingerprint_uses_stable_source_not_checkout_path() {
    use etas_code::{StableSourceKey, StableSourceOrigin};
    let analysis = analyzed("return 2;");
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay = DefaultBodyOverlay::prepare(
        analysis.frozen.clone(),
        syntax.parse_body("return 2;").unwrap(),
        &syntax,
    )
    .unwrap();
    let key = StableSourceKey::new(
        StableSourceOrigin::Package(PackageKey::from_name_and_version("dependency", "1").unwrap()),
        &["src".into(), "dep.es".into()],
    )
    .unwrap();
    let source = |path: &str, text: &str| super::super::CandidateDependencySource {
        source: SourceInput {
            id: etas_core::SourceId(10),
            path: Some(path.into()),
            text: text.into(),
            kind: crate::SourceKind::DependencySourceOverlay {
                package: crate::ExternalPackageId(7),
                import_root: "dep".into(),
            },
        },
        stable_key: key,
    };
    let fingerprint = |source| {
        let mut context = CandidatePipelineContext::new(overlay.clone());
        context.set_dependency_sources(vec![source]).unwrap();
        context.snapshot
    };
    let first = fingerprint(source("/checkout/a/src/dep.es", "module dep;"));
    assert_eq!(
        first,
        fingerprint(source("/checkout/b/src/dep.es", "module dep;"))
    );
    assert_ne!(
        first,
        fingerprint(source("/checkout/b/src/dep.es", "module dep; // changed"))
    );
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
