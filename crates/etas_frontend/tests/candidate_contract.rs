use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CodeFingerprint, PackageKey, descriptor::DescriptorLimits, syntax::CodeSyntaxOperations,
};
use etas_frontend::{
    EntryPolicy, Frontend, FrontendSession, ProjectInput, SourceInput,
    code::{
        contract::{
            CandidateContractAnalysis, ContractComponent, ContractRejection, ContractRelation,
            ContractUncertainty,
        },
        definition::{DefinitionCatalog, FrozenDefinitionEnvironment},
        draft::DefaultBodyOverlay,
        identity::DefinitionOrigins,
        service::{CandidateBodyAnalysis, NativeCodeSyntaxOperations},
    },
};

fn freeze(source: &str, remove_summaries: bool) -> Arc<FrozenDefinitionEnvironment> {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.entry_policy = EntryPolicy::Optional;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut checked = output.checked.unwrap();
    if remove_summaries {
        Arc::make_mut(&mut Arc::make_mut(&mut checked).effects)
            .item_effects
            .clear();
    }
    let package = |bytes| {
        PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
            "etas.test.contract",
            bytes,
        ))
    };
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::declared_source_modules(&checked, package(b"app"))
            .unwrap(),
        standard_package: package(b"std"),
        external_definitions: BTreeMap::new(),
    };
    let expression = checked
        .types
        .reflections
        .iter()
        .find_map(|(expression, fact)| {
            let etas_types::CheckedReflectionTarget::Agent { declaration, .. } = fact.target else {
                return None;
            };
            (checked.symbols.get(declaration)?.name == "Worker").then_some(*expression)
        })
        .unwrap();
    DefinitionCatalog::build(checked, &origins, DescriptorLimits::default())
        .unwrap()
        .freeze_agent(expression)
        .unwrap()
}

fn analyze(frozen: Arc<FrozenDefinitionEnvironment>, body: &str) -> CandidateBodyAnalysis {
    let syntax = NativeCodeSyntaxOperations::default();
    let overlay =
        DefaultBodyOverlay::prepare(frozen, syntax.parse_body(body).unwrap(), &syntax).unwrap();
    FrontendSession::new()
        .analyze_body_overlay(overlay)
        .unwrap()
}

fn relation(analysis: &CandidateBodyAnalysis, component: ContractComponent) -> &ContractRelation {
    let Some(CandidateContractAnalysis::Analyzed {
        snapshot,
        frozen_contract,
        checks,
    }) = analysis.contract_analysis()
    else {
        panic!("contract analysis: {:?}", analysis.output().diagnostics)
    };
    assert_eq!(*snapshot, analysis.fingerprint());
    assert_eq!(*frozen_contract, analysis.frozen().contract().fingerprint());
    &checks
        .iter()
        .find(|check| check.component == component)
        .unwrap()
        .relation
}

#[test]
fn handled_new_requests_do_not_fit_an_empty_frozen_action_bound() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "println(\"new request\")?; return 2;");
    assert!(analysis.output().checked.is_some());
    assert_eq!(
        relation(&analysis, ContractComponent::EscapingEffects),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Rejected(ContractRejection::AdditionalEffects)
    );
}

#[test]
fn handler_factory_trace_order_is_enforced_by_the_frozen_contract() {
    let frozen = freeze(
        r#"module app;
effect Gate { action ask() -> bool; }
let gate: ![Gate => [] for bool] = handler { Gate.ask() => { resume true; } };
flow choose(h: ![Gate => [] for bool]) -> ![Gate => [] for bool] ![Console.stdout_write] {
    perform Console.stdout_write("factory");
    return h;
}
agent Worker() -> bool ![Console.stdout_write] {
    return handle { perform Gate.ask() } with choose(gate);
}
flow inspect() -> unit { let d = ^Worker; return; }"#,
        false,
    );
    let analysis = analyze(
        frozen,
        r#"let value = handle { perform Gate.ask() } with gate;
perform Console.stdout_write("factory");
return value;"#,
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert!(matches!(
        relation(&analysis, ContractComponent::ActionTrace),
        ContractRelation::Rejected(ContractRejection::ActionTraceNotContained(_))
    ));
}

#[test]
fn candidate_cannot_change_payload_generic_instances_behind_the_same_action() {
    let frozen = freeze(
        r#"module app;
import std.compiler.{CompileError, compile};
agent First() -> i32 { return 1; }
agent Other() -> i32 { return 2; }
agent Worker() -> unit ![Error<CompileError>] { let value = compile(^First); return; }
flow inspect() -> unit { let definition = ^Worker; return; }"#,
        false,
    );
    let analysis = analyze(frozen, "let value = compile(^Other); return;");
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::EscapingEffects),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Rejected(ContractRejection::ChangedRequestInstantiation)
    );
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

#[test]
fn candidate_preserves_payload_generic_instances_across_store_relocation() {
    let frozen = freeze(
        r#"module app;
import std.compiler.{CompileError, compile};
agent First() -> i32 { return 1; }
agent Worker() -> unit ![Error<CompileError>] { let value = compile(^First); return; }
flow inspect() -> unit { let definition = ^Worker; return; }"#,
        false,
    );
    let analysis = analyze(
        frozen,
        "let unused = [1, 2]; let value = compile(^First); return;",
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
}

#[test]
fn calls_to_frozen_declarations_preserve_requested_actions_and_outcomes() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> i32 { println(\"original\")?; return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let source = frozen.checked().effects.item_effects[&frozen.default_body().unwrap()].clone();
    let analysis = analyze(frozen, "return Worker();");
    let checked = analysis.output().checked.as_ref().expect("valid candidate");
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty());
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert_eq!(summary.default_actions.effects.iter().count(), 1);
    assert_eq!(summary.action_trace, source.action_trace);
    assert!(summary.exits.as_ref().unwrap().gaps.is_empty());
    for component in [
        ContractComponent::RequestedActions,
        ContractComponent::ActionTrace,
        ContractComponent::TemporalOutcomes,
    ] {
        assert_eq!(relation(&analysis, component), &ContractRelation::Proven);
    }
    analysis.into_checked_candidate(Default::default()).unwrap();
}

#[test]
fn frozen_agent_boundaries_stop_local_execution_traversal_without_erasing_effects() {
    let frozen = freeze(
        "module app; import std.io.println; agent Helper() -> i32 { println(\"helper\")?; return 7; } agent Worker() -> i32 { println(\"original\")?; return 1; } flow inspect() -> unit { let definition = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return Helper();");
    let checked = analysis.output().checked.as_ref().unwrap();
    let entry = checked.entry.unwrap();
    let helper = checked
        .hir
        .items
        .iter()
        .find_map(|(item, declaration)| {
            let etas_hir::HirItem::Agent(declaration) = declaration else {
                return None;
            };
            (checked.symbols.get(declaration.symbol)?.name == "Helper").then_some(item)
        })
        .unwrap();
    let local = checked.checked_call_reachability(entry).unwrap();
    assert!(local.contains(&entry));
    assert!(!local.contains(&helper));
    let summary = &checked.effects.item_effects[&entry];
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert_eq!(summary.default_actions.effects.iter().count(), 1);
    assert!(summary.exits.as_ref().unwrap().gaps.is_empty());
    analysis.into_checked_candidate(Default::default()).unwrap();
}

#[test]
fn a_later_return_does_not_inherit_an_earlier_calls_value_observation() {
    let frozen = freeze(
        "module app; agent Worker() -> bool { return true; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "Worker(); return false;");
    assert!(matches!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        ContractRelation::Rejected(ContractRejection::OutcomeTraceNotContained(_))
    ));
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

#[test]
fn repeated_frozen_calls_do_not_collapse_to_a_single_action_trace() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> i32 { println(\"original\")?; return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "Worker(); return Worker();");
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
    assert!(matches!(
        relation(&analysis, ContractComponent::ActionTrace),
        ContractRelation::Rejected(ContractRejection::ActionTraceNotContained(_))
    ));
    assert!(analysis.into_checked_candidate(Default::default()).is_err());
}

#[test]
fn frozen_calls_relocate_nominal_error_selectors() {
    let frozen = freeze(
        "module app; type Failure = string; agent Worker() -> i32 ![Error<Failure>] { return perform Error<Failure>.raise(Failure(\"stop\")); } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return Worker();");
    let checked = analysis.output().checked.as_ref().expect("valid candidate");
    let row = &checked.effects.item_effects[&checked.entry.unwrap()].escaping_effects;
    let effects = row.effects.iter().cloned().collect::<Vec<_>>();
    let [etas_effects::Effect::Error(ty)] = effects.as_slice() else {
        panic!("{row:?}")
    };
    assert!(
        matches!(checked.type_store.get(*ty), Some(etas_types::Type::Nominal(nominal)) if nominal.name == "app.Failure")
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
    analysis.into_checked_candidate(Default::default()).unwrap();
}

#[test]
fn missing_original_summary_cannot_be_replaced_with_candidate_analysis() {
    let frozen = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        true,
    );
    let analysis = analyze(frozen, "return Worker();");
    assert!(analysis.output().checked.is_none());
    assert!(
        analysis
            .output()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == etas_core::DiagnosticCode::Effect(
                    etas_core::EffectDiagnosticCode::IncompleteEffectFacts
                )
                && diagnostic
                    .message
                    .contains("frozen callable has no solved effect summary")),
        "{:?}",
        analysis.output().diagnostics
    );
}

#[test]
fn same_action_sets_cannot_justify_reversed_temporal_order() {
    let frozen = freeze(
        "module app; import std.io.println; import std.io.eprintln; agent Worker() -> i32 { println(\"first\")?; eprintln(\"second\")?; return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(
        frozen,
        "eprintln(\"second\")?; println(\"first\")?; return 2;",
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
    assert!(matches!(
        relation(&analysis, ContractComponent::ActionTrace),
        ContractRelation::Rejected(ContractRejection::ActionTraceNotContained(_))
    ));
}

#[test]
fn genuine_branch_refinement_is_not_required_to_repeat_the_original_algorithm() {
    let frozen = freeze(
        "module app; import std.io.println; import std.io.eprintln; agent Worker(flag: bool) -> i32 { if flag { println(\"a\")?; } else { eprintln(\"b\")?; } return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "println(\"a\")?; return 2;");
    assert_eq!(
        relation(&analysis, ContractComponent::CallableInterface),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::ActionTrace),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
}

#[test]
fn inferred_output_changes_cannot_replace_the_frozen_interface() {
    let frozen = freeze(
        "module app; agent Worker; impl agent Worker { flow run() { return 1; } } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return \"different type\";");
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::CallableInterface),
        &ContractRelation::Rejected(ContractRejection::ChangedInterface)
    );
}

#[test]
fn missing_frozen_analysis_is_not_an_empty_contract() {
    let frozen = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        true,
    );
    let analysis = analyze(frozen, "return 2;");
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Unproven(ContractUncertainty::MissingFrozenAnalysis)
    );
}

#[test]
fn successive_checks_keep_the_same_original_contract_reference() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let contract = frozen.contract().fingerprint();
    let first = analyze(frozen, "println(\"first\")?; return 2;");
    let second = analyze(first.frozen().clone(), "println(\"second\")?; return 3;");
    assert_eq!(first.frozen().contract().fingerprint(), contract);
    assert_eq!(second.frozen().contract().fingerprint(), contract);
    assert_eq!(
        relation(&second, ContractComponent::RequestedActions),
        &ContractRelation::Rejected(ContractRejection::AdditionalEffects)
    );
}

#[test]
fn pure_literal_replacement_has_real_outcome_evidence() {
    let frozen = freeze(
        "module app; agent Worker() -> i32 { return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return 2;");
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
    let Some(CandidateContractAnalysis::Analyzed { checks, .. }) = analysis.contract_analysis()
    else {
        panic!("analysis")
    };
    assert!(
        checks
            .iter()
            .all(|check| check.relation == ContractRelation::Proven),
        "{checks:?}"
    );
}

#[test]
fn concrete_generic_data_does_not_require_returned_callable_behavior() {
    for (declarations, output) in [
        ("enum Chain<T> { End, Link(T, Chain<T>), }", "Chain<i32>"),
        (
            "type Id = string; type Envelope<T> = { value: T };",
            "Envelope<Id>",
        ),
        (
            "enum Recursive<T> { Value(T), Next(Recursive<Array<T>>), }",
            "Recursive<i32>",
        ),
    ] {
        let source = format!(
            "module app; {declarations} agent Worker(value: {output}) -> {output} {{ return value; }} flow inspect() -> unit {{ let d = ^Worker; return; }}"
        );
        let analysis = analyze(freeze(&source, false), "return value;");
        assert_eq!(
            relation(&analysis, ContractComponent::LatentBehavior),
            &ContractRelation::Proven,
            "{output}"
        );
    }
}

#[test]
fn captured_and_discarded_std_io_errors_have_checked_completion_evidence() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> i32 { println(\"original\")?; return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "println(\"candidate\")?; return 2;");
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
}

#[test]
fn observing_a_try_result_still_requires_variant_sensitive_outcome_evidence() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker() -> Result<unit, IOError> { return println(\"original\")?; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return println(\"candidate\")?;");
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Unproven(ContractUncertainty::MissingOutcomeSensitiveSummary)
    );
}

#[test]
fn a_generic_data_payload_containing_a_callable_still_requires_latent_evidence() {
    let source = "module app; enum Wrapped<T> { Value(T), } agent Worker(value: Wrapped<() -> i32>) -> Wrapped<() -> i32> { return value; } flow inspect() -> unit { let d = ^Worker; return; }";
    let analysis = analyze(freeze(source, false), "return value;");
    assert_eq!(
        relation(&analysis, ContractComponent::LatentBehavior),
        &ContractRelation::Unproven(ContractUncertainty::MissingLatentBehaviorSummary)
    );
}

#[test]
fn a_checked_raised_error_is_not_replaced_with_a_normal_return_outcome() {
    let frozen = freeze(
        "module app; agent Worker() -> i32 ![Error<string>] { return perform Error<string>.raise(\"bad\"); } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let preserved = analyze(
        frozen.clone(),
        "return perform Error<string>.raise(\"still bad\");",
    );
    assert_eq!(
        relation(&preserved, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
    let analysis = analyze(frozen, "return 1;");
    assert!(
        matches!(
            relation(&analysis, ContractComponent::TemporalOutcomes),
            ContractRelation::Rejected(ContractRejection::OutcomeTraceNotContained(_))
        ),
        "{:?}",
        relation(&analysis, ContractComponent::TemporalOutcomes)
    );
}

#[test]
fn same_empty_action_trace_does_not_erase_boolean_return_distinctions() {
    let frozen = freeze(
        "module app; agent Worker() -> bool { return true; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return false;");
    assert_eq!(
        relation(&analysis, ContractComponent::CallableInterface),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::ActionTrace),
        &ContractRelation::Proven
    );
    assert!(matches!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        ContractRelation::Rejected(ContractRejection::OutcomeTraceNotContained(_))
    ));
}

#[test]
fn return_outcome_branch_refinement_can_be_proved() {
    let frozen = freeze(
        "module app; agent Worker(flag: bool) -> bool { if flag { return true; } else { return false; } } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return true;");
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
}

#[test]
fn enum_return_observations_keep_the_checked_constructor_identity() {
    let frozen = freeze(
        "module app; enum Status { Accepted, Rejected, } agent Worker() -> Status { return Status.Accepted; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return Status.Rejected;");
    assert_eq!(
        relation(&analysis, ContractComponent::CallableInterface),
        &ContractRelation::Proven
    );
    assert!(matches!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        ContractRelation::Rejected(ContractRejection::OutcomeTraceNotContained(_))
    ));
}

#[test]
fn the_no_action_branch_is_not_dropped_from_a_frozen_trace() {
    let frozen = freeze(
        "module app; import std.io.println; agent Worker(flag: bool) -> i32 { if flag { println(\"optional\")?; } return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return 2;");
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::ActionTrace),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
}

#[test]
fn a_partial_pure_intrinsic_is_not_given_io_completion_evidence() {
    let frozen = freeze(
        "module app; import std.option.unwrap; agent Worker(value: Option<i32>) -> i32 { return unwrap(value); } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return unwrap(value);");
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Unproven(ContractUncertainty::MissingOutcomeSensitiveSummary)
    );
}

#[test]
fn checked_pure_generic_helper_calls_preserve_available_outcomes() {
    let frozen = freeze(
        "module app; flow identity<T>(value: T) -> T { return value; } agent Worker(value: i32) -> i32 { return value; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(frozen, "return identity(value);");
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
}

#[test]
fn returned_flows_require_a_latent_behavior_contract_not_just_an_empty_root_row() {
    let frozen = freeze(
        "module app; import std.io.println; alias Callback = () -> unit ![]; agent Worker() -> Callback { return () => { return; }; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(
        frozen,
        "return () => { println(\"latent request\")?; return; };",
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::RequestedActions),
        &ContractRelation::Proven
    );
    assert_eq!(
        relation(&analysis, ContractComponent::LatentBehavior),
        &ContractRelation::Unproven(ContractUncertainty::MissingLatentBehaviorSummary)
    );
}

#[test]
fn a_checked_call_with_a_constant_finishing_error_handler_preserves_outcome_evidence() {
    let frozen = freeze(
        "module app; import std.io.{println, IOError}; agent Worker() -> unit { println(\"baseline\")?; return; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(
        frozen,
        "let value = handle { println(\"candidate\") } with { Error<IOError>.raise(_) => { finish (); } }; return;",
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Proven
    );
}

#[test]
fn handler_arm_actions_are_not_erased_by_the_value_completion_proof() {
    let frozen = freeze(
        "module app; import std.io.{println, IOError}; agent Worker() -> unit { println(\"baseline\")?; return; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(
        frozen,
        "let value = handle { println(\"candidate\") } with { Error<IOError>.raise(_) => { println(\"arm\")?; finish (); } }; return;",
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Unproven(ContractUncertainty::MissingOutcomeSensitiveSummary)
    );
}

#[test]
fn fallible_arguments_do_not_inherit_the_enclosing_intrinsics_completion_proof() {
    let frozen = freeze(
        "module app; import std.io.{println, read_line, IOError}; agent Worker() -> unit { println(\"baseline\")?; return; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(
        frozen,
        "let value = handle { println(read_line()) } with { Error<IOError>.raise(_) => { finish (); } }; return;",
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Unproven(ContractUncertainty::MissingOutcomeSensitiveSummary)
    );
}

#[test]
fn a_refutable_error_pattern_cannot_claim_total_handler_completion() {
    let frozen = freeze(
        "module app; import std.compiler.{compile, AgentDef, CompileError}; agent Helper() -> i32 { return 1; } agent Worker(definition: AgentDef<Helper>) -> i32 { compile(definition)?; return 1; } flow inspect() -> unit { let d = ^Worker; return; }",
        false,
    );
    let analysis = analyze(
        frozen,
        "let next = handle { compile(definition) } with { Error<CompileError>.raise(CompileError { kind: \"rejected\", diagnostics: _ }) => { finish Helper; } }; return 2;",
    );
    assert!(
        analysis.output().checked.is_some(),
        "{:?}",
        analysis.output().diagnostics
    );
    assert_eq!(
        relation(&analysis, ContractComponent::TemporalOutcomes),
        &ContractRelation::Unproven(ContractUncertainty::MissingOutcomeSensitiveSummary)
    );
}
