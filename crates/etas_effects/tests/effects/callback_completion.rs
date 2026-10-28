use super::*;
use etas_effects::CONSOLE_STDERR_WRITE_ACTION;

#[test]
fn a_concrete_generic_effect_row_still_checks_the_lambda_body() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"module app;
flow apply<T, effect E>(f: (T) -> T ![E], value: T) -> T ![E] { return f(value); }
flow main() -> i32 ![] {
    return apply<i32, ![]>((value: i32) => { perform Console.stdout_write("forbidden"); return value; }, 42);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::EscapedEffect)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn public_factory_contract_does_not_invoke_its_returned_callable() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"module app;
public flow make<T, effect E>(f: (T) -> T ![E]) -> ((T) -> T ![E]) ![] { return (value: T) => { return f(value); }; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let contract = output
        .facts
        .public_contracts
        .iter()
        .find(|contract| contract.item == flow_item(&hir, "make"))
        .unwrap();
    assert!(contract.public_row.is_empty(), "{contract:?}");
    assert!(contract.inferred.is_empty(), "{contract:?}");
    assert!(contract.requested_actions.is_empty(), "{contract:?}");
    assert_eq!(contract.latent_flows.len(), 1);
    assert!(contract.latent_flows[0].inferred.open.is_some());
}

#[test]
fn returned_generic_closures_keep_type_and_effect_bindings_through_wrappers() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"module app;
flow make<T, effect E>(f: (T) -> T ![E]) -> ((T) -> T ![E]) ![] { return (value: T) => { return f(value); }; }
flow relay<U, effect F>(g: (U) -> U ![F]) -> ((U) -> U ![F]) ![] { return make<U, ![F]>(g); }
flow stdout(value: i32) -> i32 ![Console.stdout_write] { perform Console.stdout_write("first"); return value; }
flow stderr(value: string) -> string ![Console.stderr_write] { perform Console.stderr_write(value); return value; }
flow first() -> i32 ![Console.stdout_write] { let saved = relay<i32, ![Console.stdout_write]>(stdout); return saved(42); }
flow second() -> string ![Console.stderr_write] { let saved = relay<string, ![Console.stderr_write]>(stderr); return saved("second"); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for (name, action, ty) in [
        (
            "first",
            CONSOLE_STDOUT_WRITE_ACTION,
            etas_types::PrimitiveType::I32,
        ),
        (
            "second",
            CONSOLE_STDERR_WRITE_ACTION,
            etas_types::PrimitiveType::String,
        ),
    ] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        let expected = Effect::Action(ActionRef {
            tag: CONSOLE_TAG,
            action,
        });
        assert_eq!(
            summary.escaping_effects,
            EffectRow::closed(EffectSet::one(expected.clone())),
            "{summary:?}"
        );
        assert_eq!(
            summary.requested_actions,
            EffectRow::closed(EffectSet::one(expected.clone())),
            "{summary:?}"
        );
        assert!(
            matches!(&summary.action_trace, ActionTraceDomain::Event(event) if event.action == expected),
            "{summary:?}"
        );
        for exit in &summary.exits.as_ref().unwrap().exits {
            if let etas_effects::CallableOutcome::Returned { ty: actual, .. } = exit.outcome {
                assert_eq!(
                    types.store.get(actual),
                    Some(&etas_types::Type::Primitive(ty))
                );
            }
        }
    }
}

#[test]
fn callback_completion_obligations_require_total_checked_sources_and_keep_other_gaps() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"module app;
flow apply(f: (i32) -> i32 ![], value: i32) -> i32 ![] { return f(value); }
flow make(f: (i32) -> i32 ![]) -> ((i32) -> i32 ![]) ![] { return (value: i32) => { return f(value); }; }
flow total(value: i32) -> i32 ![] { return value; }
flow partial(value: i32) -> i32 ![] { return value + 1; }
flow safe() -> i32 ![] { return apply(total, 0); }
flow unsafe() -> i32 ![] { return apply(partial, 0); }
flow safe_capture() -> i32 ![] { let saved = make(total); return saved(0); }
flow unsafe_capture() -> i32 ![] { let saved = make(partial); return saved(0); }
flow safe_nested_capture() -> i32 ![] { return apply(make(total), 0); }
flow unsafe_nested_capture() -> i32 ![] { return apply(make(partial), 0); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for name in ["safe", "safe_capture", "safe_nested_capture"] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        let exits = summary.exits.as_ref().unwrap();
        assert!(exits.gaps.is_empty(), "{name}: {exits:?}");
        assert!(!exits.exits.is_empty());
        assert!(
            exits
                .exits
                .iter()
                .all(|exit| matches!(exit.outcome, etas_effects::CallableOutcome::Returned { .. }))
        );
    }
    for name in [
        "unsafe",
        "unsafe_capture",
        "unsafe_nested_capture",
        "partial",
    ] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        assert!(
            !summary.exits.as_ref().unwrap().gaps.is_empty(),
            "{name}: {summary:?}"
        );
    }
    let template = &output.facts.item_effects[&flow_item(&hir, "apply")];
    assert!(
        template
            .exits
            .as_ref()
            .unwrap()
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::ParameterCall { .. })),
        "{template:?}"
    );
}
