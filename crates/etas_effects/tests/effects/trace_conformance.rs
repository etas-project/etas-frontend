use super::*;

#[test]
fn trace_analysis_rejects_forged_source_spec_instances() {
    use etas_effects::trace_spec::{
        TraceSpecAnalysisInput, analyze_trace_spec_monitors, materialize_trace_spec_models,
    };

    let source = r#"
module app;
effect Probe { action read<T>(value: T) -> unit; }
spec Safe<R>: trace = +Probe.read<R>;
flow main() -> unit ~ Safe<i32> { return; }
flow other() -> unit ~ Safe<bool> { return; }
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let main = flow_item(&hir, "main");
    let other = flow_item(&hir, "other");
    let source_id = hir.items.get(main).unwrap().span().source;
    let key = etas_code::StableSourceKey::new(
        etas_code::StableSourceOrigin::Project(etas_code::CodeFingerprint::from_bytes([7; 32])),
        &["app.es".into()],
    )
    .unwrap();
    let snapshots = [etas_effects::EffectSourceSnapshot {
        source: source_id,
        key,
        text: source,
    }];
    let registry = EffectRegistry::from_hir_and_types(&hir, &types).unwrap();
    let mut effects = etas_effects::EffectOutput::default();
    let models =
        materialize_trace_spec_models(&hir, &types, &registry, &[], &snapshots, &mut effects)
            .unwrap();
    assert!(effects.diagnostics.is_empty(), "{:?}", effects.diagnostics);
    let fact = &effects.facts.trace_specs.conformances[&main][0];
    assert!(fact.source_spec_instance.is_some());
    let other_instance = effects.facts.trace_specs.conformances[&other][0]
        .source_spec_instance
        .clone();
    assert_ne!(fact.source_spec_instance, other_instance);
    effects
        .facts
        .item_effects
        .insert(main, EffectSummary::local());
    effects
        .facts
        .item_effects
        .insert(other, EffectSummary::local());
    let units = [EffectUnit::Item(main), EffectUnit::Item(other)];
    let revision = etas_hir::HirRevision::try_new(hir.clone()).unwrap();
    let input = || TraceSpecAnalysisInput {
        source_snapshots: &snapshots,
        external_trace_specs: &[],
        hir: &revision,
        types: &types,
        registry: &registry,
        models: &models,
        units: &units,
        external_summaries: &[],
    };
    let mut valid = effects.clone();
    assert!(analyze_trace_spec_monitors(input(), &mut valid).is_ok());
    let mut missing_model = models.clone();
    missing_model.referenced_by_item.remove(&main);
    missing_model.referenced_fact_indices.remove(&main);
    assert!(matches!(
        analyze_trace_spec_monitors(
            TraceSpecAnalysisInput { models: &missing_model, ..input() },
            &mut effects.clone(),
        ),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("no analysis model")
    ));
    let mut both_missing = effects.clone();
    both_missing.facts.trace_specs.conformances.remove(&main);
    assert!(matches!(
        analyze_trace_spec_monitors(
            TraceSpecAnalysisInput { models: &missing_model, ..input() },
            &mut both_missing,
        ),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("no model or rejection")
    ));
    let mut forged = effects.clone();
    forged
        .facts
        .trace_specs
        .conformances
        .get_mut(&main)
        .unwrap()[0]
        .source_spec_instance = None;
    assert!(matches!(
        analyze_trace_spec_monitors(input(), &mut forged),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("checked source instance")
    ));
    let mut missing = effects.clone();
    missing.facts.trace_specs.conformances.remove(&main);
    assert!(matches!(
        analyze_trace_spec_monitors(input(), &mut missing),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("fact is missing")
    ));
    let mut duplicate = effects.clone();
    let fact = duplicate.facts.trace_specs.conformances[&main][0].clone();
    duplicate
        .facts
        .trace_specs
        .conformances
        .get_mut(&main)
        .unwrap()
        .push(fact);
    assert!(matches!(
        analyze_trace_spec_monitors(input(), &mut duplicate),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("duplicate")
    ));
    let mut wrong_spec = effects;
    wrong_spec
        .facts
        .trace_specs
        .conformances
        .get_mut(&main)
        .unwrap()[0]
        .source_spec_instance = other_instance;
    assert!(matches!(
        analyze_trace_spec_monitors(input(), &mut wrong_spec),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("checked source instance")
    ));

    let mut invalid_types = types.clone();
    std::sync::Arc::make_mut(&mut invalid_types.facts)
        .spec_signatures
        .clear();
    let mut invalid_effects = etas_effects::EffectOutput::default();
    let invalid_models = materialize_trace_spec_models(
        &hir,
        &invalid_types,
        &registry,
        &[],
        &snapshots,
        &mut invalid_effects,
    )
    .unwrap();
    assert!(!invalid_effects.diagnostics.is_empty());
    let invalid_input = || TraceSpecAnalysisInput {
        types: &invalid_types,
        models: &invalid_models,
        ..input()
    };
    let mut reported = invalid_effects.clone();
    assert!(analyze_trace_spec_monitors(invalid_input(), &mut reported).is_ok());
    assert!(!reported.diagnostics.is_empty());
    invalid_effects.diagnostics.clear();
    assert!(matches!(
        analyze_trace_spec_monitors(invalid_input(), &mut invalid_effects),
        Err(etas_effects::EffectPipelineError::InvalidTraceSpecSourceSites { reason })
            if reason.contains("no matching error diagnostic")
    ));
}

#[test]
fn attached_trace_specs_are_conjoined_without_flattening_their_internal_alternatives() {
    for conformance in [
        "Blocked + Permitted",
        "(-Console.stdout_write | +Console.stdout_write) + (-Console.stdout_write)",
    ] {
        let source = format!(
            r#"
module app;
import std.io.println;
spec Blocked: trace = -Console.stdout_write;
spec Permitted: trace = +Console.stdout_write;
flow main() -> unit ~ {conformance} {{ println("handled")?; return; }}
"#
        );
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let mut types = etas_types::check_program(&hir);
        add_std_symbol_type_facts(&hir, &mut types);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let output = check_program(&hir, &types);
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
            "{conformance}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn closed_row_callback_requests_cannot_bypass_a_trace_spec() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
spec Quiet: trace = -Console.stdout_write;
flow twice(f: () -> unit ![]) -> unit { f(); f(); }
flow relay(g: () -> unit ![]) -> unit { twice(g); }
flow main() -> unit ~ Quiet {
    relay(() => { println("handled")?; return; });
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let summary = &output.facts.item_effects[&flow_item(&hir, "main")];
    let stdout = Effect::Action(ActionRef {
        tag: CONSOLE_TAG,
        action: CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(summary.escaping_effects.is_empty(), "{summary:?}");
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.default_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn a_proven_nonreturning_callee_does_not_execute_the_following_trace() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
spec Quiet: trace = -Console.stdout_write;
flow failure() -> i32 ![Error<string>] { return perform Error<string>.raise("stop"); }
flow main() -> i32 ![Error<string>] ~ Quiet {
    failure();
    println("unreachable")?;
    return 1;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let item = hir
        .items
        .iter()
        .find_map(|(item, declaration)| {
            let HirItem::Flow(flow) = declaration else {
                return None;
            };
            (hir.symbols.get(flow.symbol)?.name == "main").then_some(item)
        })
        .unwrap();
    let summary = &output.facts.item_effects[&item];
    assert!(summary.requested_actions.is_empty(), "{summary:?}");
    assert!(summary.exits.as_ref().unwrap().gaps.is_empty());
    assert!(
        summary
            .exits
            .as_ref()
            .unwrap()
            .exits
            .iter()
            .all(|exit| matches!(exit.outcome, etas_effects::CallableOutcome::Raised { .. }))
    );
}

#[test]
fn missing_checked_trace_conformance_cannot_remove_the_declared_constraint() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
import std.io.println;
spec Safe: trace = -Console.stdout_write;
flow main() -> unit ![Error<IOError>] ~ Safe { println("forbidden"); return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    assert_eq!(types.facts.trace_spec_conformances.len(), 1);
    std::sync::Arc::make_mut(&mut types.facts)
        .trace_spec_conformances
        .clear();
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
        "{:?}",
        output.diagnostics
    );
}
