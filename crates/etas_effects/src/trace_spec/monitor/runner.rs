use std::collections::BTreeSet;

use etas_utils::automaton::{ProductAutomaton, ProductState, SymbolicAutomaton};

use crate::{ActionEvent, ActionTraceDomain, EffectRegistry, EffectSet};

use crate::trace_spec::model::TraceSpecPattern;

use super::compiler::{
    TemporalState, TraceSpecLabel, require_after_automaton, require_before_automaton,
};
use super::matcher::TraceSpecMatcher;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalMonitorResult {
    Satisfied,
    Violation,
    Unknown,
}

pub fn require_before(
    trace: &ActionTraceDomain,
    guard: &TraceSpecPattern,
    target: &TraceSpecPattern,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
) -> TemporalMonitorResult {
    let monitor = SingleComponentMonitor::new(require_before_automaton(guard, target));
    monitor.run(
        trace,
        registry,
        types,
        |state| matches!(state, TemporalState::Rejected),
        |actions| {
            actions
                .iter()
                .any(|action| target.matches_effect(action, registry, types))
        },
    )
}

pub fn require_after(
    trace: &ActionTraceDomain,
    target: &TraceSpecPattern,
    obligation: &TraceSpecPattern,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
) -> TemporalMonitorResult {
    let monitor = SingleComponentMonitor::new(require_after_automaton(target, obligation));
    monitor.run(
        trace,
        registry,
        types,
        |state| matches!(state, TemporalState::Pending),
        |actions| {
            actions
                .iter()
                .any(|action| target.matches_effect(action, registry, types))
        },
    )
}

struct SingleComponentMonitor {
    product: ProductAutomaton<u8, TemporalState, TraceSpecLabel>,
}

impl SingleComponentMonitor {
    fn new(automaton: SymbolicAutomaton<TemporalState, TraceSpecLabel>) -> Self {
        let mut product = ProductAutomaton::new();
        product.insert(0, automaton);
        Self { product }
    }

    fn run(
        &self,
        trace: &ActionTraceDomain,
        registry: &EffectRegistry,
        types: &etas_types::TypeOutput,
        final_rejects: impl Fn(TemporalState) -> bool,
        unknown_needs_order: impl Fn(&EffectSet) -> bool,
    ) -> TemporalMonitorResult {
        let mut states = BTreeSet::from([self.product.initial_state()]);
        let matcher = TraceSpecMatcher { registry, types };
        match self.run_trace(trace, states, &matcher, &unknown_needs_order) {
            TraceRun::Violation => TemporalMonitorResult::Violation,
            TraceRun::Unknown => TemporalMonitorResult::Unknown,
            TraceRun::States(next) => {
                states = next;
                if states
                    .iter()
                    .any(|state| component_state(state).is_some_and(&final_rejects))
                {
                    TemporalMonitorResult::Violation
                } else {
                    TemporalMonitorResult::Satisfied
                }
            }
        }
    }

    fn run_trace(
        &self,
        trace: &ActionTraceDomain,
        states: BTreeSet<ProductState<u8, TemporalState>>,
        matcher: &TraceSpecMatcher<'_>,
        unknown_needs_order: &impl Fn(&EffectSet) -> bool,
    ) -> TraceRun {
        match trace {
            ActionTraceDomain::Empty => TraceRun::States(states),
            ActionTraceDomain::Event(event) => self.step_event(states, event, matcher),
            ActionTraceDomain::Seq(parts) => {
                let mut current = states;
                for part in parts {
                    match self.run_trace(part, current, matcher, unknown_needs_order) {
                        TraceRun::States(next) => current = next,
                        other => return other,
                    }
                }
                TraceRun::States(current)
            }
            ActionTraceDomain::Choice(branches) => {
                let mut joined = BTreeSet::new();
                for branch in branches {
                    match self.run_trace(branch, states.clone(), matcher, unknown_needs_order) {
                        TraceRun::States(next) => joined.extend(next),
                        other => return other,
                    }
                }
                TraceRun::States(joined)
            }
            ActionTraceDomain::Repeat(inner) => {
                let actions = inner.action_set();
                if unknown_needs_order(&actions) {
                    TraceRun::Unknown
                } else {
                    TraceRun::States(states)
                }
            }
            ActionTraceDomain::UnknownOrder(actions) => {
                if unknown_needs_order(actions) {
                    TraceRun::Unknown
                } else {
                    TraceRun::States(states)
                }
            }
        }
    }

    fn step_event(
        &self,
        states: BTreeSet<ProductState<u8, TemporalState>>,
        event: &ActionEvent,
        matcher: &TraceSpecMatcher<'_>,
    ) -> TraceRun {
        let mut next_states = BTreeSet::new();
        for state in states {
            let Ok(step) = self.product.step(&state, event, matcher) else {
                return TraceRun::Unknown;
            };
            for next in step.states {
                if component_state(&next).is_some_and(|state| state == TemporalState::Rejected) {
                    return TraceRun::Violation;
                }
                next_states.insert(next);
            }
        }
        TraceRun::States(next_states)
    }
}

enum TraceRun {
    States(BTreeSet<ProductState<u8, TemporalState>>),
    Violation,
    Unknown,
}

fn component_state(state: &ProductState<u8, TemporalState>) -> Option<TemporalState> {
    state.get(&0).copied()
}
