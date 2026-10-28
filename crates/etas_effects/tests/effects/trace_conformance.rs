use super::*;

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
