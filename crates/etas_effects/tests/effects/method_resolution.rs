use super::*;

#[test]
fn method_owner_and_local_binders_specialize_distinct_effects_and_trace_events() {
    assert_owner_effects(
        include_str!("../fixtures/method_resolution/owner_binders.es"),
        &["main"],
    );
}

#[test]
fn returned_method_closure_keeps_owner_and_shadowing_local_effect_binders() {
    assert_owner_effects(
        include_str!("../fixtures/method_resolution/captured_owner_binders.es"),
        &["main", "wrapped"],
    );
}

fn assert_owner_effects(source: &str, entries: &[&str]) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let output = check_program(&hir, &types);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let template = output
        .facts
        .performed_actions
        .values()
        .find(|fact| fact.effect_segments == ["Probe"] && fact.action == "hit")
        .unwrap();
    let Effect::AppliedAction(action) = &template.invocation else {
        panic!("typed action");
    };
    let event = |primitive| {
        let ty = types
            .store
            .iter()
            .find_map(|(ty, data)| (*data == etas_types::Type::Primitive(primitive)).then_some(ty))
            .unwrap();
        Effect::AppliedAction(etas_effects::ActionInstanceRef {
            action: action.action.clone(),
            args: vec![etas_types::EffectArgRef::Type(ty)],
        })
    };
    let owner = event(etas_types::PrimitiveType::I32);
    let local = event(etas_types::PrimitiveType::String);
    for name in entries {
        let summary = &output.facts.item_effects[&flow_item(&hir, name)];
        let expected = [owner.clone(), local.clone()].into_iter().collect();
        assert_eq!(summary.escaping_effects.effects, expected, "{name}");
        assert_eq!(summary.requested_actions.effects, expected, "{name}");
        let ActionTraceDomain::Seq(events) = &summary.action_trace else {
            panic!("ordered calls for {name}: {:?}", summary.action_trace);
        };
        assert!(
            matches!(&events[..], [ActionTraceDomain::Event(first), ActionTraceDomain::Event(second)]
            if first.action == owner && second.action == local),
            "{name}: {events:?}"
        );
    }
}

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
