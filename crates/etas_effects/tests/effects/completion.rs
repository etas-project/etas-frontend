use super::*;

#[test]
fn checked_limit_constructors_have_complete_outcomes() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow limits() -> unit {
    let iterations = Iterations(8);
    let tokens = Tokens(9);
    let context = ContextTokens(10);
    let attempts = Attempts(11);
    return;
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
    let exits = output.facts.item_effects[&flow_item(&hir, "limits")]
        .exits
        .as_ref()
        .unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    assert!(!exits.exits.is_empty(), "{exits:?}");
}

#[test]
fn checked_bounded_for_preserves_complete_pure_loop_outcomes() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow looped(values: Array<bool>) -> unit {
    for value in values limit Iterations(8) { let copy = value; }
    return;
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
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(exits.gaps.is_empty(), "{exits:?}");
    assert!(!exits.exits.is_empty(), "{exits:?}");
    assert!(
        !exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::ExternalCall(_))),
        "{exits:?}"
    );
}

#[test]
fn checked_bounded_for_does_not_hide_unproved_body_operations() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow looped(values: Array<i32>) -> unit {
    for value in values limit Iterations(3) { let next = value + 1; }
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Operation(_))),
        "{exits:?}"
    );
}

#[test]
fn checked_bounded_for_keeps_break_outcomes_unproven() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow looped(values: Array<bool>) -> unit {
    for value in values limit Iterations(3) { if value { break; } }
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Loop(_))),
        "{exits:?}"
    );
}

#[test]
fn generic_iteration_element_does_not_prove_callable_absence() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        "module app; flow looped<T>(values: Array<T>) -> unit { for value in values limit Iterations(3) { let copy = value; } return; }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    let exits = output.facts.item_effects[&flow_item(&hir, "looped")]
        .exits
        .as_ref()
        .unwrap();
    assert!(
        exits
            .gaps
            .iter()
            .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Loop(_))),
        "{exits:?}"
    );
}

#[test]
fn checked_boolean_operations_and_count_queries_preserve_complete_return_summaries() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow logic(a: bool, b: bool) -> bool { return !(a == b) && (a != b || a); }
flow size(values: Array<i32>) -> usize { return values.len(); }
flow emptiness(values: Array<i32>) -> bool { return values.is_empty(); }
flow arithmetic(value: i8) -> i8 { return value + 1; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let mut types = etas_types::check_program(&hir);
    add_std_symbol_type_facts(&hir, &mut types);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    for (item, declaration) in hir.items.iter() {
        let HirItem::Flow(flow) = declaration else {
            continue;
        };
        let name = &hir.symbols.get(flow.symbol).unwrap().name;
        let exits = output.facts.item_effects[&item].exits.as_ref().unwrap();
        if name == "arithmetic" {
            assert!(
                exits
                    .gaps
                    .iter()
                    .any(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::Operation(_))),
                "{exits:?}"
            );
        } else {
            assert!(exits.gaps.is_empty(), "{name}: {exits:?}");
            assert!(!exits.exits.is_empty(), "{name}: missing return summary");
        }
    }
}
