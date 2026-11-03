use std::collections::BTreeSet;

use etas_code::residual::monitor::{
    PriorEvidenceLabel, PriorEvidenceState, compile_prior_evidence,
};
use etas_utils::automaton::{ProductAutomaton, ProductState, ProductStepLimits, SymbolicAutomaton};

use crate::{ActionEvent, ActionTraceDomain, EffectRegistry, EffectSet};

use crate::trace_spec::model::TraceSpecPattern;

use super::matcher::{TraceObservation, TraceSpecMatcher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum TemporalMonitorResult {
    Satisfied,
    Violation,
    /// Relevant ordering is known, but an action request cannot prove
    /// the outcome, correlation or provenance of the required evidence.
    NeedsOutcomeEvidence,
    Unknown,
}

pub fn require_before(
    trace: &ActionTraceDomain,
    guard: &TraceSpecPattern,
    target: &TraceSpecPattern,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
) -> TemporalMonitorResult {
    let monitor = SingleComponentMonitor::new(compile_prior_evidence(guard, target), guard, target);
    let result = monitor.run(
        trace,
        registry,
        types,
        |state| matches!(state, PriorEvidenceState::Rejected),
        |actions| {
            actions.iter().any(|action| {
                target.may_match_effect(action, registry, types)
                    || guard.may_match_effect(action, registry, types)
            })
        },
    );
    // ActionTraceDomain records requested actions but not a correlated guard
    // outcome. Ordered requests alone cannot prove accepted guard evidence.
    if result == TemporalMonitorResult::Satisfied
        && trace
            .action_set()
            .iter()
            .any(|action| target.matches_effect(action, registry, types))
    {
        TemporalMonitorResult::NeedsOutcomeEvidence
    } else {
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AbstractState {
    monitor: ProductState<u8, PriorEvidenceState>,
    prior_guard_request: bool,
}

struct SingleComponentMonitor<'a> {
    product: ProductAutomaton<u8, PriorEvidenceState, PriorEvidenceLabel<TraceSpecPattern>>,
    guard: &'a TraceSpecPattern,
    target: &'a TraceSpecPattern,
}

impl<'a> SingleComponentMonitor<'a> {
    fn new(
        automaton: SymbolicAutomaton<PriorEvidenceState, PriorEvidenceLabel<TraceSpecPattern>>,
        guard: &'a TraceSpecPattern,
        target: &'a TraceSpecPattern,
    ) -> Self {
        let mut product = ProductAutomaton::new();
        product.insert(0, automaton);
        Self {
            product,
            guard,
            target,
        }
    }

    fn run(
        &self,
        trace: &ActionTraceDomain,
        registry: &EffectRegistry,
        types: &etas_types::TypeOutput,
        final_rejects: impl Fn(PriorEvidenceState) -> bool,
        unknown_needs_order: impl Fn(&EffectSet) -> bool,
    ) -> TemporalMonitorResult {
        let states = BTreeSet::from([AbstractState {
            monitor: self.product.initial_state(),
            prior_guard_request: false,
        }]);
        let matcher = TraceSpecMatcher { registry, types };
        match self.run_trace(trace, states, &matcher, &unknown_needs_order) {
            TraceRun::Violation => TemporalMonitorResult::Violation,
            TraceRun::States { states, incomplete } => {
                if states
                    .iter()
                    .any(|state| component_state(&state.monitor).is_some_and(&final_rejects))
                {
                    TemporalMonitorResult::Violation
                } else if incomplete {
                    TemporalMonitorResult::Unknown
                } else {
                    TemporalMonitorResult::Satisfied
                }
            }
        }
    }

    fn run_trace(
        &self,
        trace: &ActionTraceDomain,
        states: BTreeSet<AbstractState>,
        matcher: &TraceSpecMatcher<'_>,
        unknown_needs_order: &impl Fn(&EffectSet) -> bool,
    ) -> TraceRun {
        if states.is_empty() {
            return TraceRun::known(states);
        }
        match trace {
            ActionTraceDomain::Empty => TraceRun::known(states),
            ActionTraceDomain::Event(event) => self.step_event(states, event, matcher),
            ActionTraceDomain::ParameterCall { .. } => TraceRun::unknown(),
            ActionTraceDomain::Seq(parts) => {
                let mut current = states;
                let mut incomplete = false;
                for part in parts {
                    match self.run_trace(part, current, matcher, unknown_needs_order) {
                        TraceRun::States {
                            states: next,
                            incomplete: next_incomplete,
                        } => {
                            current = next;
                            incomplete |= next_incomplete;
                        }
                        TraceRun::Violation => return TraceRun::Violation,
                    }
                }
                TraceRun::States {
                    states: current,
                    incomplete,
                }
            }
            ActionTraceDomain::Choice(branches) => {
                if branches.is_empty() {
                    return TraceRun::unknown();
                }
                let mut joined = BTreeSet::new();
                let mut incomplete = false;
                for branch in branches {
                    match self.run_trace(branch, states.clone(), matcher, unknown_needs_order) {
                        TraceRun::States {
                            states: next,
                            incomplete: next_incomplete,
                        } => {
                            joined.extend(next);
                            incomplete |= next_incomplete;
                        }
                        TraceRun::Violation => return TraceRun::Violation,
                    }
                }
                TraceRun::States {
                    states: joined,
                    incomplete,
                }
            }
            ActionTraceDomain::Repeat(inner) => {
                let actions = inner.action_set();
                if !inner.has_parameter_calls() && !unknown_needs_order(&actions) {
                    return TraceRun::known(states);
                }
                // Nested Kleene closures have the same language as one closure.
                let mut inner = inner.as_ref();
                while let ActionTraceDomain::Repeat(nested) = inner {
                    inner = nested;
                }
                let mut reached = states;
                let mut pending = reached.clone();
                let mut incomplete = false;
                while !pending.is_empty() {
                    match self.run_trace(inner, pending, matcher, unknown_needs_order) {
                        TraceRun::Violation => return TraceRun::Violation,
                        TraceRun::States {
                            states: next,
                            incomplete: next_incomplete,
                        } => {
                            incomplete |= next_incomplete;
                            pending = next.difference(&reached).cloned().collect();
                            reached.extend(pending.iter().cloned());
                        }
                    }
                }
                TraceRun::States {
                    states: reached,
                    incomplete,
                }
            }
            ActionTraceDomain::UnknownOrder(actions) => {
                if unknown_needs_order(actions) {
                    TraceRun::unknown()
                } else {
                    TraceRun::known(states)
                }
            }
            ActionTraceDomain::Widened {
                actions,
                parameter_calls,
            } => {
                if !parameter_calls.is_empty() || unknown_needs_order(actions) {
                    TraceRun::unknown()
                } else {
                    TraceRun::known(states)
                }
            }
        }
    }

    fn step_event(
        &self,
        states: BTreeSet<AbstractState>,
        event: &ActionEvent,
        matcher: &TraceSpecMatcher<'_>,
    ) -> TraceRun {
        if [self.guard, self.target].iter().any(|pattern| {
            !pattern.matches_event(event, matcher.registry, matcher.types)
                && pattern.may_match_effect(&event.action, matcher.registry, matcher.types)
        }) {
            return TraceRun::unknown();
        }
        let mut next_states = BTreeSet::new();
        let mut incomplete = false;
        for state in states {
            let observation = TraceObservation {
                event,
                prior_guard_request: state.prior_guard_request,
            };
            let limits = ProductStepLimits {
                max_components: 1,
                max_product_states: 1,
                max_work: 3,
            };
            let Ok(step) = self
                .product
                .step_bounded(&state.monitor, &observation, matcher, limits)
            else {
                incomplete = true;
                continue;
            };
            if step.states.is_empty() || !step.rejecting.is_empty() {
                return TraceRun::Violation;
            }
            for next in step.states {
                if component_state(&next).is_some_and(|state| state == PriorEvidenceState::Rejected)
                {
                    return TraceRun::Violation;
                }
                next_states.insert(AbstractState {
                    monitor: next,
                    prior_guard_request: state.prior_guard_request
                        || self
                            .guard
                            .matches_event(event, matcher.registry, matcher.types),
                });
            }
        }
        TraceRun::States {
            states: next_states,
            incomplete,
        }
    }
}

enum TraceRun {
    States {
        states: BTreeSet<AbstractState>,
        incomplete: bool,
    },
    Violation,
}

impl TraceRun {
    fn known(states: BTreeSet<AbstractState>) -> Self {
        Self::States {
            states,
            incomplete: false,
        }
    }

    fn unknown() -> Self {
        Self::States {
            states: BTreeSet::new(),
            incomplete: true,
        }
    }
}

fn component_state(state: &ProductState<u8, PriorEvidenceState>) -> Option<PriorEvidenceState> {
    state.get(&0).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActionEventSource, Effect, EffectRow, ParameterCallTarget};
    use etas_core::{SourceId, Span, TextRange, TextSize};

    #[test]
    fn repeated_traces_use_zero_or_more_iteration_fixpoint() {
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let guard = Effect::Action(registry.action_by_name("Console.stdin_read_line").unwrap());
        let target = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let unrelated = Effect::Action(registry.action_by_name("Console.stderr_write").unwrap());
        let pattern = |action: Effect| TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(action)),
            label: String::new(),
            arg_bounds: Vec::new(),
        };
        let event = |action| {
            ActionTraceDomain::Event(ActionEvent {
                action,
                span: Span::new(SourceId(0), TextRange::new(TextSize(0), TextSize(0))),
                source: ActionEventSource::Perform,
            })
        };
        let repeat = |trace| ActionTraceDomain::Repeat(Box::new(trace));
        let check = |trace: &ActionTraceDomain| {
            require_before(
                trace,
                &pattern(guard.clone()),
                &pattern(target.clone()),
                &registry,
                &types,
            )
        };

        assert_eq!(
            check(&repeat(event(target.clone()))),
            TemporalMonitorResult::Violation
        );
        assert_eq!(
            check(&repeat(ActionTraceDomain::Seq(vec![
                event(guard.clone()),
                event(target.clone())
            ]))),
            TemporalMonitorResult::NeedsOutcomeEvidence
        );
        assert_eq!(
            check(&repeat(event(unrelated))),
            TemporalMonitorResult::Satisfied
        );
        assert_eq!(
            check(&repeat(ActionTraceDomain::Choice(vec![
                event(guard.clone()),
                event(target.clone())
            ]))),
            TemporalMonitorResult::Violation
        );
    }

    #[test]
    fn empty_product_transition_is_not_a_successful_monitor_result() {
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let guard = TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(action.clone())),
            label: String::new(),
            arg_bounds: Vec::new(),
        };
        let automaton = SymbolicAutomaton::new(
            PriorEvidenceState::Active,
            etas_utils::automaton::NoTransitionPolicy::Empty,
        );
        let monitor = SingleComponentMonitor::new(automaton, &guard, &guard);
        let trace = ActionTraceDomain::Event(ActionEvent {
            action,
            span: Span::new(SourceId(0), TextRange::new(TextSize(0), TextSize(0))),
            source: ActionEventSource::Perform,
        });
        assert_eq!(
            monitor.run(
                &trace,
                &registry,
                &types,
                |state| state == PriorEvidenceState::Rejected,
                |_| false
            ),
            TemporalMonitorResult::Violation
        );
    }

    #[test]
    fn empty_choice_is_not_proof_of_a_safe_trace() {
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let pattern = |name| TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(Effect::Action(
                registry.action_by_name(name).unwrap(),
            ))),
            label: String::new(),
            arg_bounds: Vec::new(),
        };
        assert_eq!(
            require_before(
                &ActionTraceDomain::Choice(Vec::new()),
                &pattern("Console.stdin_read_line"),
                &pattern("Console.stdout_write"),
                &registry,
                &types
            ),
            TemporalMonitorResult::Unknown
        );
    }

    #[test]
    fn ordered_requests_do_not_prove_correlated_success() {
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name| Effect::Action(registry.action_by_name(name).unwrap());
        let target_action = action("Console.stdout_write");
        let guard_action = action("Console.stdin_read_line");
        let pattern = |action: Effect| TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(action)),
            label: String::new(),
            arg_bounds: Vec::new(),
        };
        let target = pattern(target_action.clone());
        let guard = pattern(guard_action.clone());
        let event = |action| {
            ActionTraceDomain::Event(ActionEvent {
                action,
                span: Span::new(SourceId(0), TextRange::new(TextSize(0), TextSize(0))),
                source: ActionEventSource::Perform,
            })
        };

        assert_eq!(
            require_before(
                &ActionTraceDomain::Seq(vec![event(guard_action), event(target_action)]),
                &guard,
                &target,
                &registry,
                &types,
            ),
            TemporalMonitorResult::NeedsOutcomeEvidence
        );
        assert_eq!(
            require_before(
                &ActionTraceDomain::Empty,
                &guard,
                &target,
                &registry,
                &types,
            ),
            TemporalMonitorResult::Satisfied
        );

        let same_action = pattern(action("Console.stdout_write"));
        assert_eq!(
            require_before(
                &event(action("Console.stdout_write")),
                &same_action,
                &same_action,
                &registry,
                &types,
            ),
            TemporalMonitorResult::Violation,
            "an action cannot provide its own earlier evidence"
        );
    }

    #[test]
    fn static_and_concrete_prior_monitors_agree_where_evidence_is_known() {
        use etas_code::{
            CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey, RevisionKey,
            contract::CanonicalEffect,
            descriptor::EffectTargetInfo,
            residual::{
                binding::{BindingLayout, BindingLayoutLimits, BorrowedBindingLimits},
                identity::{ActivationSiteId, SemanticScopeId},
                monitor::{
                    AcceptedPriorEvidence, ActionAttemptId, ConcreteActionAttempt,
                    ConcreteEvidenceLimits, ConcretePriorEvidence, EvidenceQuery, EvidenceStatus,
                    PriorEvidenceStep, ScopeInvocationId, SemanticEventCursor,
                    step_concrete_prior_evidence,
                },
                pattern::{ActionPattern, ActionPatternLimits},
            },
        };
        struct Evidence(EvidenceStatus);
        impl ConcretePriorEvidence for Evidence {
            fn query(&self, _: EvidenceQuery<'_, '_>) -> EvidenceStatus {
                self.0
            }
        }
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let static_action = |name| Effect::Action(registry.action_by_name(name).unwrap());
        let guard_action = static_action("Console.stdin_read_line");
        let target_action = static_action("Console.stdout_write");
        let static_pattern = |action| TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(action)),
            label: String::new(),
            arg_bounds: Vec::new(),
        };
        let span = Span::new(SourceId(0), TextRange::new(TextSize(0), TextSize(0)));
        let event = |action| {
            ActionTraceDomain::Event(ActionEvent {
                action,
                span,
                source: ActionEventSource::Perform,
            })
        };
        let guard = static_pattern(guard_action.clone());
        let target = static_pattern(target_action.clone());
        assert_eq!(
            require_before(
                &event(target_action.clone()),
                &guard,
                &target,
                &registry,
                &types
            ),
            TemporalMonitorResult::Violation
        );
        assert_eq!(
            require_before(
                &ActionTraceDomain::Seq(vec![event(guard_action), event(target_action)]),
                &guard,
                &target,
                &registry,
                &types
            ),
            TemporalMonitorResult::NeedsOutcomeEvidence
        );
        let fp = CodeFingerprint::from_bytes([1; 32]);
        let owner = DefinitionKey::new(
            PackageKey::from_canonical_fingerprint(fp),
            vec!["std".into()],
            vec!["Console".into()],
            DefinitionKind::Effect,
        )
        .unwrap();
        let canonical = |name: &str| CanonicalEffect {
            target: EffectTargetInfo::Action {
                effect: owner.clone(),
                name: name.into(),
            },
            arguments: vec![],
        };
        let limits = ActionPatternLimits {
            max_selectors: 2,
            max_identity_segments: 8,
            max_identity_bytes: 80,
            max_literal_bytes: 80,
        };
        let checked_guard = ActionPattern::new(canonical("stdin_read_line"), limits).unwrap();
        let checked_target = ActionPattern::new(canonical("stdout_write"), limits).unwrap();
        let write = canonical("stdout_write");
        let bindings = Default::default();
        let layout = BindingLayout::new(
            Default::default(),
            BindingLayoutLimits {
                max_bindings: 0,
                max_projections_per_binding: 0,
                max_total_projections: 0,
            },
        )
        .unwrap();
        let attempt = ConcreteActionAttempt {
            scope: SemanticScopeId::from_canonical_fingerprint(fp),
            activation: ActivationSiteId::from_canonical_fingerprint(fp),
            revision: RevisionKey::from_canonical_fingerprint(fp),
            invocation: ScopeInvocationId::from_bytes([1; 16]),
            id: ActionAttemptId::from_bytes([2; 16]),
            cursor: SemanticEventCursor(2),
            action: &write,
            bindings: layout
                .validate_borrowed(
                    &bindings,
                    BorrowedBindingLimits {
                        max_bindings: 2,
                        max_value_bytes: 80,
                        max_total_value_bytes: 160,
                    },
                )
                .unwrap(),
        };
        let evidence_limits = ConcreteEvidenceLimits {
            pattern: limits,
            max_bindings: 2,
            max_value_bytes: 80,
        };
        for (status, expected) in [
            (EvidenceStatus::Absent, PriorEvidenceStep::Rejected),
            (EvidenceStatus::Denied, PriorEvidenceStep::Rejected),
            (
                EvidenceStatus::Accepted(AcceptedPriorEvidence {
                    scope: attempt.scope,
                    activation: attempt.activation,
                    revision: attempt.revision,
                    invocation: attempt.invocation,
                    guard_pattern: checked_guard.id(),
                    target_pattern: checked_target.id(),
                    guard_attempt: ActionAttemptId::from_bytes([3; 16]),
                    completed_at: SemanticEventCursor(1),
                }),
                PriorEvidenceStep::Active,
            ),
        ] {
            assert_eq!(
                step_concrete_prior_evidence(
                    &checked_guard,
                    &checked_target,
                    PriorEvidenceState::Active,
                    &attempt,
                    &Evidence(status),
                    evidence_limits
                ),
                Ok(expected)
            );
        }
    }

    #[test]
    fn unknown_choice_does_not_hide_a_definite_violation() {
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name| Effect::Action(registry.action_by_name(name).unwrap());
        let target_action = action("Console.stdout_write");
        let guard_action = action("Console.stdin_read_line");
        let pattern = |action: Effect| TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(action)),
            label: String::new(),
            arg_bounds: Vec::new(),
        };
        let span = Span::new(SourceId(0), TextRange::new(TextSize(0), TextSize(0)));
        let unknown = ActionTraceDomain::ParameterCall {
            target: ParameterCallTarget::Interface(0),
            parameter: "callback".to_owned(),
            span,
        };
        let violation = ActionTraceDomain::Event(ActionEvent {
            action: target_action.clone(),
            span,
            source: ActionEventSource::Perform,
        });
        for branches in [
            vec![unknown.clone(), violation.clone()],
            vec![violation.clone(), unknown.clone()],
        ] {
            assert_eq!(
                require_before(
                    &ActionTraceDomain::Choice(branches),
                    &pattern(guard_action.clone()),
                    &pattern(target_action.clone()),
                    &registry,
                    &types,
                ),
                TemporalMonitorResult::Violation
            );
        }

        assert_eq!(
            require_before(
                &ActionTraceDomain::Seq(vec![
                    ActionTraceDomain::Choice(vec![unknown.clone(), ActionTraceDomain::Empty]),
                    violation.clone(),
                ]),
                &pattern(guard_action.clone()),
                &pattern(target_action.clone()),
                &registry,
                &types,
            ),
            TemporalMonitorResult::Violation
        );

        assert_eq!(
            require_before(
                &ActionTraceDomain::Repeat(Box::new(unknown)),
                &pattern(guard_action.clone()),
                &pattern(target_action.clone()),
                &registry,
                &types,
            ),
            TemporalMonitorResult::Unknown
        );

        let repeated_guard =
            ActionTraceDomain::Repeat(Box::new(ActionTraceDomain::Event(ActionEvent {
                action: guard_action.clone(),
                span,
                source: ActionEventSource::Perform,
            })));
        assert_eq!(
            require_before(
                &ActionTraceDomain::Seq(vec![repeated_guard, violation]),
                &pattern(guard_action),
                &pattern(target_action),
                &registry,
                &types,
            ),
            TemporalMonitorResult::Violation
        );
    }
}
