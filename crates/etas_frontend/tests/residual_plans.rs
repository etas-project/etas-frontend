//! End-to-end residual plan production through the checked canonical
//! projection and the selected-target support gate.

use std::collections::BTreeSet;

use etas_code::{
    PackageKey,
    residual::target::{
        ResidualComponentKind, ResidualEventPhase, ResidualEvidenceKind, ResidualScopeExitKind,
        ResidualSuspensionKind, ResidualTargetCapabilities, ResidualTargetLimits,
    },
};
use etas_core::{DiagnosticCode, EffectDiagnosticCode};
use etas_frontend::{Frontend, ProjectInput, ProjectOutput, SourceInput};

const ORDERED: &str = r#"
module app;
effect Gate { action check() -> unit; action send() -> unit; action extra() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> unit ![Gate.check, Gate.send] ~ Safe { perform Gate.check(); perform Gate.send(); return; }
"#;

const WRONG_ORDER: &str = r#"
module app;
effect Gate { action check() -> unit; action send() -> unit; }
spec Safe: trace = Gate.check >> Gate.send;
flow main() -> unit ![Gate.check, Gate.send] ~ Safe { perform Gate.send(); perform Gate.check(); return; }
"#;

fn supporting_target() -> ResidualTargetCapabilities {
    ResidualTargetCapabilities {
        engine: "test-engine".into(),
        engine_semantic_version: 1,
        plan_versions: BTreeSet::from([(1, 1)]),
        trace_template_versions: BTreeSet::from([(1, 1)]),
        components: BTreeSet::from([
            ResidualComponentKind::ActionPredicate,
            ResidualComponentKind::TraceAlternatives,
        ]),
        phases: BTreeSet::from([
            ResidualEventPhase::PreDispatchAttempt,
            ResidualEventPhase::CorrelatedOutcome,
        ]),
        evidence: BTreeSet::from([ResidualEvidenceKind::AcceptedPriorOutcome]),
        exits: ResidualScopeExitKind::ALL.into_iter().collect(),
        suspension: ResidualSuspensionKind::ALL.into_iter().collect(),
        limits: ResidualTargetLimits {
            max_obligations_per_plan: 64,
            max_alternatives: 64,
            max_clauses: 64,
            max_patterns: 64,
        },
    }
}

fn check(
    source: &str,
    stable_source: bool,
    target: Option<ResidualTargetCapabilities>,
) -> ProjectOutput {
    Frontend.check_project(input(source, stable_source, target))
}

fn input(
    source: &str,
    stable_source: bool,
    target: Option<ResidualTargetCapabilities>,
) -> ProjectInput {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    if let Some(target) = target {
        input.options.residual_target = target;
    }
    if stable_source {
        let package = PackageKey::from_name_and_version("app", "1.0.0").unwrap();
        input.environment.local_source_package = Some(package);
        let key = etas_code::StableSourceKey::new(
            etas_code::StableSourceOrigin::Package(package),
            &["src".into(), "app.es".into()],
        )
        .unwrap();
        input
            .environment
            .stable_source_keys
            .push(etas_frontend::ProjectStableSourceKeyInput {
                source: input.sources[0].id,
                key,
            });
    }
    input
}

const GUARDED_FLOW_BODY: &str = "handle { perform Gate.check(); perform Gate.send(); } with { Gate.check() => { resume (); } Gate.send() => { resume (); } }; return;";
const HANDLER_ACTION_FLOW_BODY: &str = "handle { perform Gate.check(); perform Gate.send(); } with { Gate.check() => { handle { perform Audit.note(); } with { Audit.note() => { resume (); } }; resume (); } Gate.send() => { resume (); } }; return;";

fn reflected_guarded_flow_analysis(
    prefix: &str,
    through_callee: bool,
) -> etas_frontend::code::service::CandidateBodyAnalysis {
    let (body, conformance, callee) = if through_callee {
        (
            "guarded(); return;",
            "",
            format!("flow guarded() -> unit ![] ~ Safe {{ {GUARDED_FLOW_BODY} }}"),
        )
    } else {
        (GUARDED_FLOW_BODY, "~ Safe", String::new())
    };
    let source = format!(
        "module app; effect Gate {{ action check() -> unit; action send() -> unit; }} spec Safe: trace = Gate.check >> Gate.send; {callee} flow main() -> unit ![] {conformance} {{ {body} }} flow inspect() -> unit {{ let definition = ^main; return; }}"
    );
    reflected_source_flow_analysis(
        &source,
        (!prefix.is_empty())
            .then(|| format!("{prefix}{body}"))
            .as_deref(),
    )
}

fn reflected_source_flow_analysis(
    source: &str,
    replacement: Option<&str>,
) -> etas_frontend::code::service::CandidateBodyAnalysis {
    use etas_code::{definition::CodeDefinitionOperations, syntax::CodeSyntaxOperations};
    use etas_frontend::{
        FrontendSession,
        code::{
            definition::{DefinitionCatalog, NativeDefinitionStore, ReflectionOptions},
            identity::DefinitionOrigins,
            service::NativeCodeSyntaxOperations,
        },
    };
    use std::{collections::BTreeMap, sync::Arc};
    let mut input = input(source, true, Some(supporting_target()));
    input.options.reflection = ReflectionOptions {
        retain_bodies: vec!["app.main".into()],
        export_bodies: vec![],
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let expression = *checked.types.reflections.keys().next().unwrap();
    let package = PackageKey::from_name_and_version("app", "1.0.0").unwrap();
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
        standard_package: package,
        external_definitions: BTreeMap::new(),
    };
    let catalog =
        Arc::new(DefinitionCatalog::build(checked, &origins, Default::default()).unwrap());
    let store = Arc::new(NativeDefinitionStore::new(
        catalog,
        Arc::new(NativeCodeSyntaxOperations::default()),
        Default::default(),
    ));
    let original = store.reflect_flow(expression).unwrap();
    let replacement = match replacement {
        None => store.resolve(&original).unwrap().body().unwrap().clone(),
        Some(body) => store.syntax_operations().parse_body(body).unwrap(),
    };
    let draft = store.with_body(&original, replacement).unwrap();
    let overlay = store
        .resolve(&draft)
        .unwrap()
        .body_overlay(store.syntax_operations())
        .unwrap();
    FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
}

#[test]
fn edited_flow_proves_handler_action_traces_without_reusing_the_original_body() {
    use etas_code::report::CheckStatus;
    let body = HANDLER_ACTION_FLOW_BODY;
    let source = format!(
        "module app; effect Gate {{ action check() -> unit; action send() -> unit; }} effect Audit {{ action note() -> unit; }} flow main() -> unit {{ {body} }} flow inspect() -> unit {{ let definition = ^main; return; }}"
    );
    let analysis =
        reflected_source_flow_analysis(&source, Some(&format!("let edited = true; {body}")));
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Accepted,
        "{:#?}",
        report.diagnostics
    );
    analysis.into_checked_candidate(Default::default()).unwrap();

    let reordered = "handle { perform Gate.check(); perform Gate.send(); } with { Gate.check() => { resume (); } Gate.send() => { handle { perform Audit.note(); } with { Audit.note() => { resume (); } }; resume (); } }; return;";
    let analysis = reflected_source_flow_analysis(&source, Some(reordered));
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(
        report.diagnostics.iter().any(|diagnostic| diagnostic.code
            == "code::ActionTraceNotContained"
            || diagnostic.code == "code::OutcomeTraceNotContained"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::MissingOutcomeSensitiveSummary"),
        "{:#?}",
        report.diagnostics
    );
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

#[test]
fn guarded_flow_and_trace_spec_share_the_applied_handler_action_trace() {
    use etas_code::report::CheckStatus;
    let body = HANDLER_ACTION_FLOW_BODY;
    let source = format!(
        "module app; effect Gate {{ action check() -> unit; action send() -> unit; }} effect Audit {{ action note() -> unit; }} spec Safe: trace = (Gate.check >> Gate.send) & +Audit.note; flow main() -> unit ~ Safe {{ {body} }} flow inspect() -> unit {{ let definition = ^main; return; }}"
    );
    let analysis =
        reflected_source_flow_analysis(&source, Some(&format!("let edited = true; {body}")));
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Guarded,
        "{:#?}",
        report.diagnostics
    );
    assert!(!report.runtime_checks.is_empty());
    analysis.into_checked_candidate(Default::default()).unwrap();
}

#[test]
fn reflected_flow_with_verified_residual_plan_emits_a_guarded_candidate() {
    use etas_code::report::CheckStatus;
    let analysis = reflected_guarded_flow_analysis("", false);
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        CheckStatus::Guarded,
        "{:#?}",
        report.diagnostics
    );
    assert!(!report.runtime_checks.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity
                != etas_code::diagnostic::DiagnosticSeverity::Error),
        "{:#?}",
        report.diagnostics
    );
    report.validate(Default::default()).unwrap();
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    let binding = candidate
        .residual_binding()
        .expect("guarded candidate needs its final revision binding");
    binding
        .validate(candidate.program().residual_plans())
        .unwrap();
    assert_eq!(
        binding.revision(candidate.entry()).unwrap(),
        candidate.revision()
    );
}

#[test]
fn edited_flow_with_total_resuming_handlers_has_fresh_guarded_outcomes() {
    use etas_code::report::CheckStatus;
    for through_callee in [false, true] {
        let before = reflected_guarded_flow_analysis("", through_callee)
            .check_report(Default::default())
            .unwrap();
        let analysis = reflected_guarded_flow_analysis("let edited = true;", through_callee);
        let report = analysis.check_report(Default::default()).unwrap();
        assert_ne!(report.snapshot_fingerprint, before.snapshot_fingerprint);
        assert_eq!(
            report.status,
            CheckStatus::Guarded,
            "{:#?}",
            report.diagnostics
        );
        assert!(!report.runtime_checks.is_empty());
        let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
        candidate
            .residual_binding()
            .unwrap()
            .validate(candidate.program().residual_plans())
            .unwrap();
    }
}

#[test]
fn residual_plan_never_discharges_missing_outcome_sensitive_compatibility() {
    use etas_code::report::CheckStatus;
    let analysis = reflected_guarded_flow_analysis("let uncertain = 1 + 2;", false);
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, CheckStatus::Rejected);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::MissingOutcomeSensitiveSummary")
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "code::UnsupportedResidualChecks")
    );
    assert!(!report.runtime_checks.is_empty());
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

#[test]
fn transitive_guarded_call_is_reported_and_bound_to_its_own_revision() {
    let analysis = reflected_guarded_flow_analysis("", true);
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(
        report.status,
        etas_code::report::CheckStatus::Guarded,
        "{:#?}",
        report.diagnostics
    );
    assert!(!report.runtime_checks.is_empty());
    let candidate = analysis.into_checked_candidate(Default::default()).unwrap();
    let binding = candidate.residual_binding().unwrap();
    binding
        .validate(candidate.program().residual_plans())
        .unwrap();
    assert!(binding.revision(candidate.entry()).is_none());
    for owner in candidate.program().residual_plans().items() {
        let revision = binding.revision(owner).unwrap();
        assert_ne!(owner, candidate.entry());
        assert_ne!(revision.key(), candidate.revision().key());
    }
}

#[test]
fn verified_runtime_plan_never_accepts_a_known_bad_edited_action_order() {
    let analysis = reflected_guarded_flow_analysis(
        "handle { perform Gate.send(); } with { Gate.send() => { resume (); } };",
        false,
    );
    let report = analysis.check_report(Default::default()).unwrap();
    assert_eq!(report.status, etas_code::report::CheckStatus::Rejected);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "effect::TraceSpecRequirementNotDominating"),
        "{:#?}",
        report.diagnostics
    );
    assert!(report.runtime_checks.is_empty());
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

fn codes(output: &ProjectOutput) -> Vec<EffectDiagnosticCode> {
    output
        .diagnostics
        .iter()
        .filter_map(|diagnostic| match &diagnostic.code {
            DiagnosticCode::Effect(code) => Some(code.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn verified_plan_without_target_support_is_rejected_at_the_backend_gate() {
    let output = check(ORDERED, true, None);
    let codes = codes(&output);
    assert!(
        codes.contains(&EffectDiagnosticCode::TraceSpecResidualUnsupported),
        "{:#?}",
        output.diagnostics
    );
    // The conformance was discharged by a verified plan, so the static
    // missing-fact rejection is replaced by the precise backend rejection.
    assert!(
        !codes.contains(&EffectDiagnosticCode::IncompleteEffectFacts),
        "{:#?}",
        output.diagnostics
    );
    assert!(output.checked.is_none());
}

#[test]
fn missing_source_identity_keeps_the_static_rejection() {
    let output = check(ORDERED, false, Some(supporting_target()));
    let codes = codes(&output);
    assert!(codes.contains(&EffectDiagnosticCode::IncompleteEffectFacts));
    assert!(!codes.contains(&EffectDiagnosticCode::TraceSpecResidualUnsupported));
    assert!(output.checked.is_none());
}

#[test]
fn supporting_target_accepts_only_the_verified_obligation() {
    let output = check(ORDERED, true, Some(supporting_target()));
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.expect("checked program");
    let main = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(decl)
                if checked.hir.symbols.get(decl.symbol).unwrap().name == "main" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let summary = &checked.effects.item_effects[&main];
    assert_eq!(summary.residual_checks.iter().count(), 1);
    assert!(!matches!(
        summary.support,
        etas_effects::InterpreterSupport::Rejected(_)
    ));

    // Identity is deterministic across compilations.
    let again = check(ORDERED, true, Some(supporting_target()))
        .checked
        .unwrap();
    assert!(
        again.effects.item_effects[&main]
            .residual_checks
            .iter()
            .eq(summary.residual_checks.iter())
    );
}

#[test]
fn partial_target_support_reports_the_missing_semantics() {
    let mut no_evidence = supporting_target();
    no_evidence.evidence.clear();
    let mut no_cancel = supporting_target();
    no_cancel.exits.remove(&ResidualScopeExitKind::Cancellation);
    let mut no_restore = supporting_target();
    no_restore
        .suspension
        .remove(&ResidualSuspensionKind::CheckpointRestore);
    let mut small = supporting_target();
    small.limits.max_obligations_per_plan = 0;
    for (target, expected) in [
        (no_evidence, "Evidence(AcceptedPriorOutcome)"),
        (no_cancel, "Exit(Cancellation)"),
        (no_restore, "Suspension(CheckpointRestore)"),
        (small, "ObligationLimit"),
    ] {
        let output = check(ORDERED, true, Some(target));
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecResidualUnsupported)
            })
            .unwrap_or_else(|| panic!("{:#?}", output.diagnostics));
        assert!(
            diagnostic.message.contains(expected),
            "{}",
            diagnostic.message
        );
        assert!(output.checked.is_none());
    }
}

#[test]
fn definite_violation_is_never_a_residual_even_with_target_support() {
    let output = check(WRONG_ORDER, true, Some(supporting_target()));
    let codes = codes(&output);
    assert!(codes.contains(&EffectDiagnosticCode::TraceSpecRequirementNotDominating));
    assert!(!codes.contains(&EffectDiagnosticCode::TraceSpecResidualUnsupported));
    assert!(output.checked.is_none());
}

#[test]
fn residual_target_is_a_compile_option_identity_input() {
    let base = etas_frontend::ProjectCompileOptions::default();
    let mut supported = base.clone();
    supported.residual_target = supporting_target();
    let mut other = supported.clone();
    other.residual_target.engine_semantic_version += 1;
    let fingerprints = [&base, &supported, &other]
        .map(|options| options.canonical_options_fingerprint())
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(fingerprints.len(), 3);
}

fn main_item(checked: &etas_frontend::CheckedProgramData) -> etas_hir::HirItemId {
    checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(decl)
                if checked.hir.symbols.get(decl.symbol).unwrap().name == "main" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap()
}

fn gate_action(checked: &etas_frontend::CheckedProgramData, name: &str) -> etas_effects::Effect {
    let action = checked
        .effect_registry
        .action_by_name(&format!("Gate.{name}"))
        .unwrap_or_else(|| panic!("Gate.{name}"));
    etas_effects::Effect::Action(action)
}

#[test]
fn checked_program_carries_bound_plans_and_shared_action_identity() {
    let output = check(ORDERED, true, Some(supporting_target()));
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.expect("checked program");
    let main = main_item(&checked);
    let residual = checked.residual_plans();
    assert_eq!(residual.target(), &supporting_target());
    let plans = residual.item_plans(main).collect::<Vec<_>>();
    assert_eq!(plans.len(), 1);
    let plan = plans[0];
    assert_eq!(
        residual
            .item_references(main)
            .into_iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        checked.effects.item_effects[&main]
            .residual_checks
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
    );
    let binding = residual.binding().expect("ordinary binding");
    binding.validate(residual).expect("binding validates");
    let revision = binding.revision(main).expect("owner revision");
    let bound = binding.bound(plan.type_fact_index()).expect("bound plan");
    assert_eq!(bound.data().plan, plan.plan().hash());
    assert_eq!(bound.data().revision, revision.key());

    // Concrete action identities agree with the plan's projected patterns.
    let obligation = plan.plan().obligations().iter().next().unwrap().id();
    let Some(etas_code::residual::plan::SemanticPlanComponent::Trace(component)) =
        plan.plan().component_for(obligation)
    else {
        panic!("trace component");
    };
    let etas_code::residual::monitor::TraceComponentMonitor::Concrete(monitor) =
        component.monitor()
    else {
        panic!("concrete trace component");
    };
    let covered = |name: &str| {
        let effect = residual
            .canonical_action(&checked, main, &gate_action(&checked, name))
            .unwrap();
        monitor
            .patterns()
            .values()
            .any(|pattern| pattern.covers(&effect))
    };
    assert!(covered("check"));
    assert!(covered("send"));
    assert!(!covered("extra"));

    // Recompilation reproduces the identical binding.
    let again = check(ORDERED, true, Some(supporting_target()))
        .checked
        .unwrap();
    let again_binding = again.residual_plans().binding().unwrap();
    assert_eq!(again_binding.revision(main).unwrap(), revision);
    assert_eq!(
        again_binding.bound(plan.type_fact_index()).unwrap().data(),
        bound.data()
    );
    for _ in 0..16 {
        let repeated = check(ORDERED, true, Some(supporting_target()))
            .checked
            .unwrap();
        let binding = repeated.residual_plans().binding().unwrap();
        assert_eq!(binding.revision(main).unwrap(), revision);
        assert_eq!(
            binding.bound(plan.type_fact_index()).unwrap().data(),
            bound.data()
        );
    }
}

#[test]
fn bound_revision_depends_on_the_selected_target() {
    let first = check(ORDERED, true, Some(supporting_target()))
        .checked
        .unwrap();
    let mut other_target = supporting_target();
    other_target.engine_semantic_version += 1;
    let second = check(ORDERED, true, Some(other_target)).checked.unwrap();
    let main = main_item(&first);
    let first_revision = first
        .residual_plans()
        .binding()
        .unwrap()
        .revision(main)
        .unwrap();
    let second_revision = second
        .residual_plans()
        .binding()
        .unwrap()
        .revision(main)
        .unwrap();
    assert_ne!(first_revision.key(), second_revision.key());
}

#[test]
fn programs_without_residual_obligations_carry_no_plans() {
    let source = r#"
module app;
effect Gate { action check() -> unit; action send() -> unit; }
flow main() -> unit ![Gate.check, Gate.send] { perform Gate.check(); perform Gate.send(); return; }
"#;
    let output = check(source, true, Some(supporting_target()));
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert!(checked.residual_plans().is_empty());
    assert!(checked.residual_plans().binding().is_none());
}
