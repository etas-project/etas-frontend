use super::*;

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
