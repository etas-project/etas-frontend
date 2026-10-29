use super::*;

#[test]
fn mutually_exclusive_completion_counts_join_without_losing_scope_checks() {
    let mut finished = EffectState::for_unit(EffectUnit::Item(HirItemId(2)));
    finished.handler_depth = 1;
    let mut resumed = finished.clone();
    resumed.resume_count = 1;
    let mut joined = finished.clone();
    assert!(joined.join_assign(&resumed));
    assert_eq!(joined.resume_count, 1);
    assert!(!joined.incomplete);
    assert!(finished.less_equal(&joined));
    assert!(resumed.less_equal(&joined));
    assert!(!joined.join_assign(&resumed));
    resumed.handler_depth = 2;
    assert!(joined.join_assign(&resumed));
    assert!(joined.incomplete);
}

#[test]
fn known_empty_latent_facts_are_distinct_from_absence_during_fixpoint_join() {
    let expr = HirExprId(3);
    let symbol = SymbolId(7);
    let mut known = LatentEnv::default();
    known.record_value_sources(expr, []);
    known.record_symbol_sources(symbol, []);
    let mut joined = LatentEnv::default();
    assert!(!known.less_equal(&joined));
    assert!(joined.join_assign(&known));
    assert_eq!(joined.value_sources(expr), Some(&BTreeSet::new()));
    assert_eq!(joined.symbol_sources(symbol), Some(&BTreeSet::new()));
    assert!(known.less_equal(&joined));
    assert!(!joined.join_assign(&known));
}
