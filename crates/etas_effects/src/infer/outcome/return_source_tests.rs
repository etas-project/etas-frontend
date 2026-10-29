use super::*;
use std::collections::BTreeSet;

#[test]
fn parameter_exit_prefixes_join_without_discarding_the_unknown_outcome() {
    let target = crate::ParameterCallTarget::Native(etas_hir::SymbolId(1));
    let span = etas_core::Span::new(
        etas_core::SourceId(7),
        etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(1)),
    );
    let mut left = CallableExitSummary::default();
    left.gaps.insert(OutcomeAnalysisGap::ParameterCall {
        target,
        output: TypeId(3),
        source: span.into(),
    });
    left.record_parameter_exit(ParameterExitFact {
        target,
        output: TypeId(3),
        source: span.into(),
        arguments: Some(Vec::new()),
        handler_free: Some(true),
        enclosing_handled: Some(crate::EffectRow::empty()),
        prefix: ActionTraceDomain::Empty,
    });
    let mut right = CallableExitSummary::default();
    right.record_parameter_exit(ParameterExitFact {
        target,
        output: TypeId(3),
        source: span.into(),
        arguments: Some(Vec::new()),
        handler_free: Some(true),
        enclosing_handled: Some(crate::EffectRow::empty()),
        prefix: ActionTraceDomain::ParameterCall {
            target,
            parameter: "f".into(),
            span,
        },
    });
    left.join_assign(&right);
    assert_eq!(left.parameter_exits.len(), 1);
    assert!(
        matches!(&left.parameter_exits[0].prefix, ActionTraceDomain::Choice(parts) if parts.len() == 2 && parts.contains(&ActionTraceDomain::Empty))
    );
    assert!(!left.returns_only());
    assert_eq!(left.gaps.len(), 1);
    left.record_parameter_exit(ParameterExitFact {
        target,
        output: TypeId(3),
        source: span.into(),
        arguments: None,
        handler_free: None,
        enclosing_handled: None,
        prefix: ActionTraceDomain::Empty,
    });
    assert_eq!(
        left.parameter_exits.len(),
        2,
        "missing argument evidence is not a zero-argument call"
    );
    left.gaps.clear();
    left.record(
        CallableOutcome::Returned {
            ty: TypeId(3),
            observation: ReturnObservation::Unit,
        },
        ActionTraceDomain::Empty,
    );
    assert!(
        !left.returns_only(),
        "a dangling symbolic exit must remain unproved"
    );
}

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
        handler_gaps: Default::default(),
        parameter_exits: Vec::new(),
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

#[test]
fn handler_finish_origins_are_distinct_from_returns_and_never_known_empty() {
    let mut summary = CallableExitSummary::default();
    assert!(summary.handler_finish_sources().is_none());
    let origin = CallableReturnSource::SourceItem(etas_hir::HirItemId(3));
    summary.record(
        CallableOutcome::Returned {
            ty: TypeId(4),
            observation: ReturnObservation::CallableSources(BTreeSet::from([origin.clone()])),
        },
        ActionTraceDomain::Empty,
    );
    assert!(summary.handler_finish_sources().is_none());
    summary.record(
        CallableOutcome::HandlerFinished {
            ty: TypeId(4),
            observation: ReturnObservation::CallableSources(BTreeSet::from([origin.clone()])),
        },
        ActionTraceDomain::Empty,
    );
    assert_eq!(
        summary.handler_finish_sources(),
        Some(BTreeSet::from([origin]))
    );
    summary.record(
        CallableOutcome::HandlerFinished {
            ty: TypeId(4),
            observation: ReturnObservation::Any,
        },
        ActionTraceDomain::Empty,
    );
    assert!(summary.handler_finish_sources().is_none());
}
