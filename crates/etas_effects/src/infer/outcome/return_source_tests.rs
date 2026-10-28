use super::*;
use std::collections::BTreeSet;

#[test]
fn known_callable_values_do_not_erase_unknown_control_or_supply_missing_values() {
    let source = CallableReturnSource::SourceItem(etas_hir::HirItemId(3));
    let mut summary = CallableExitSummary {
        exits: vec![ExitTraceFact {
            outcome: CallableOutcome::Returned {
                ty: TypeId(4),
                observation: ReturnObservation::CallableSources(BTreeSet::from([source.clone()])),
            },
            trace: ActionTraceDomain::Empty,
        }],
        gaps: BTreeSet::from([OutcomeAnalysisGap::ExternalCall(HirExprId(8))]),
    };
    let before = summary.clone();
    assert_eq!(
        summary.callable_return_sources(),
        Some(BTreeSet::from([source]))
    );
    assert_eq!(summary, before);
    assert!(!summary.returns_only());
    summary
        .gaps
        .insert(OutcomeAnalysisGap::MissingReturnObservation);
    assert!(summary.callable_return_sources().is_none());
    summary.gaps.clear();
    let CallableOutcome::Returned { observation, .. } = &mut summary.exits[0].outcome else {
        unreachable!()
    };
    *observation = ReturnObservation::Any;
    assert!(summary.callable_return_sources().is_none());
}
