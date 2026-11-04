use super::*;

#[test]
fn method_chains_follow_checked_bodies_and_keep_effect_trace_order() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(include_str!(
        "../fixtures/method_resolution/effectful_chain.es"
    )));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    assert_eq!(types.facts.checked_method_calls.len(), 6);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let action = |name| {
        output
            .facts
            .performed_actions
            .values()
            .find(|fact| fact.effect_segments == ["Audit"] && fact.action == name)
            .unwrap()
            .invocation
            .clone()
    };
    for name in ["main", "explicit", "generic"] {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        let expected = [action("inherent"), action("selected")]
            .into_iter()
            .collect();
        assert_eq!(summary.escaping_effects.effects, expected);
        assert_eq!(summary.requested_actions.effects, expected);
        let ActionTraceDomain::Seq(events) = &summary.action_trace else {
            panic!(
                "expected the selected bodies' sequential trace: {:?}",
                summary.action_trace
            );
        };
        assert!(
            matches!(&events[..], [ActionTraceDomain::Event(first), ActionTraceDomain::Event(second)]
            if first.action == action("inherent") && second.action == action("selected"))
        );
    }
}
