use etas_utils::automaton::{NoTransitionPolicy, SymbolicAutomaton};

use crate::trace_spec::model::TraceSpecPattern;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TemporalState {
    Start,
    Armed,
    Pending,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TraceSpecLabel {
    BeforeGuard(TraceSpecPattern),
    BeforeTargetWithoutGuard {
        guard: TraceSpecPattern,
        target: TraceSpecPattern,
    },
    AfterTarget(TraceSpecPattern),
    AfterObligationWithoutTarget {
        target: TraceSpecPattern,
        obligation: TraceSpecPattern,
    },
}

pub(super) fn require_before_automaton(
    guard: &TraceSpecPattern,
    target: &TraceSpecPattern,
) -> SymbolicAutomaton<TemporalState, TraceSpecLabel> {
    let mut automaton = SymbolicAutomaton::new(TemporalState::Start, NoTransitionPolicy::Stay);
    automaton.add_transition(
        TemporalState::Start,
        TemporalState::Armed,
        TraceSpecLabel::BeforeGuard(guard.clone()),
    );
    automaton.add_transition(
        TemporalState::Start,
        TemporalState::Rejected,
        TraceSpecLabel::BeforeTargetWithoutGuard {
            guard: guard.clone(),
            target: target.clone(),
        },
    );
    automaton.mark_rejecting(TemporalState::Rejected);
    automaton
}

pub(super) fn require_after_automaton(
    target: &TraceSpecPattern,
    obligation: &TraceSpecPattern,
) -> SymbolicAutomaton<TemporalState, TraceSpecLabel> {
    let mut automaton = SymbolicAutomaton::new(TemporalState::Start, NoTransitionPolicy::Stay);
    automaton.add_transition(
        TemporalState::Start,
        TemporalState::Pending,
        TraceSpecLabel::AfterTarget(target.clone()),
    );
    automaton.add_transition(
        TemporalState::Pending,
        TemporalState::Pending,
        TraceSpecLabel::AfterTarget(target.clone()),
    );
    automaton.add_transition(
        TemporalState::Pending,
        TemporalState::Start,
        TraceSpecLabel::AfterObligationWithoutTarget {
            target: target.clone(),
            obligation: obligation.clone(),
        },
    );
    automaton
}
