use super::*;
use etas_utils::JoinSemiLattice;

fn fixture() -> (
    crate::EffectRegistry,
    etas_types::TypeInterner,
    Effect,
    Effect,
) {
    let registry = crate::EffectRegistry::with_standard_effects();
    let mut types = etas_types::TypeInterner::new();
    let error = Effect::Error(types.primitive(etas_types::PrimitiveType::I32));
    let action = Effect::Action(registry.action_by_name("Code.check").unwrap());
    (registry, types, action, error)
}

#[test]
fn widening_keeps_default_provenance_and_an_independent_action() {
    let (registry, types, guard, _) = fixture();
    let action = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
    let span = etas_core::Span::new(
        etas_core::SourceId(0),
        etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(1)),
    );
    let mut body = EffectSummary::local();
    body.record_requested_action(action.clone());
    body.record_action_trace_event(action.clone(), span, crate::ActionEventSource::Perform);
    let mut summary = EffectSummary::local();
    summary.record_requested_action(guard.clone());
    summary.record_default_handled_action(guard.clone());
    summary.record_action_trace_event(guard.clone(), span, crate::ActionEventSource::Perform);
    summary
        .record_default_behavior(guard.clone(), &body)
        .unwrap();
    for _ in 0..40 {
        summary.record_requested_action(action.clone());
        summary.record_action_trace_event(action.clone(), span, crate::ActionEventSource::Perform);
    }
    summary.stabilize_for_fixpoint(None);
    assert!(summary.action_trace.has_unordered_approximation());
    let json = serde_json::to_string(&summary).unwrap();
    let restored: EffectSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, summary);
    summary
        .subtract_handled_effects(
            &EffectCoverage {
                registry: &registry,
                types: types.store(),
            },
            &EffectRow::closed(EffectSet::one(guard.clone())),
        )
        .unwrap();
    assert_eq!(
        summary.action_trace.action_set(),
        EffectSet::from_iter([guard, action.clone()])
    );
    assert!(summary.requested_actions.effects.contains(&action));
    assert!(
        summary
            .default_behavior
            .as_ref()
            .unwrap()
            .guarded
            .is_empty()
    );
}

#[test]
fn overriding_default_does_not_remove_an_independent_error_of_the_same_type() {
    let (registry, types, action, error) = fixture();
    let coverage = EffectCoverage {
        registry: &registry,
        types: types.store(),
    };
    let mut summary = EffectSummary::local();
    summary
        .record_default_handler_effects(
            action.clone(),
            EffectRow::closed(EffectSet::one(error.clone())),
        )
        .unwrap();
    summary.record_escaping_effect(error.clone());
    summary
        .subtract_handled_effects(&coverage, &EffectRow::closed(EffectSet::one(action)))
        .unwrap();
    assert_eq!(
        summary.escaping_effects,
        EffectRow::closed(EffectSet::one(error))
    );
    assert!(summary.default_handler_effects.is_none());
}

#[test]
fn error_capture_changes_the_contribution_without_forgetting_the_provider() {
    let (registry, types, action, error) = fixture();
    let coverage = EffectCoverage {
        registry: &registry,
        types: types.store(),
    };
    let mut summary = EffectSummary::local();
    summary
        .record_default_handler_effects(
            action.clone(),
            EffectRow::closed(EffectSet::one(error.clone())),
        )
        .unwrap();
    summary.remove_escaping_effect(&error);
    assert!(summary.escaping_effects.is_empty());
    assert_eq!(
        summary.default_handler_effects.as_ref().unwrap().handlers[0].action,
        action
    );
    summary.record_escaping_effect(error.clone());
    summary
        .subtract_handled_effects(&coverage, &EffectRow::closed(EffectSet::one(action)))
        .unwrap();
    assert!(summary.escaping_effects.effects.contains(&error));
}

#[test]
fn branch_join_keeps_default_and_direct_causes_and_is_idempotent() {
    let (registry, types, action, error) = fixture();
    let coverage = EffectCoverage {
        registry: &registry,
        types: types.store(),
    };
    let mut default = EffectSummary::local();
    default
        .record_default_handler_effects(
            action.clone(),
            EffectRow::closed(EffectSet::one(error.clone())),
        )
        .unwrap();
    let mut direct = EffectSummary::local();
    direct.record_escaping_effect(error.clone());
    let mut forward = default.clone();
    forward.join_assign(&direct);
    direct.join_assign(&default);
    assert_eq!(forward, direct);
    assert!(!forward.join_assign(&direct));
    forward
        .subtract_handled_effects(&coverage, &EffectRow::closed(EffectSet::one(action)))
        .unwrap();
    assert!(forward.escaping_effects.effects.contains(&error));
}

#[test]
fn non_action_cannot_be_registered_as_a_default_provider() {
    let (_, _, _, error) = fixture();
    let mut summary = EffectSummary::local();
    assert!(
        summary
            .record_default_handler_effects(error, EffectRow::empty())
            .is_err()
    );
    assert_eq!(summary, EffectSummary::local());
}
