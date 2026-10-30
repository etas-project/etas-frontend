use std::collections::{BTreeMap, BTreeSet};

use etas_core::Span;
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::{Effect, EffectRow, EffectSet, RequirementSet};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EffectSummary {
    pub handler_requests: super::RequestInstantiationSummary,
    #[serde(default)]
    pub default_handler_effects: Option<super::DefaultHandlerEffects>,
    #[serde(default)]
    pub exits: Option<super::CallableExitSummary>,
    pub escaping_effects: EffectRow,
    pub requested_actions: EffectRow,
    pub default_actions: EffectRow,
    pub handled_actions: EffectRow,
    pub action_trace: ActionTraceDomain,
    pub residual_checks: ResidualCheckSet,
    pub trace_spec_obligations: RequirementSet,
    pub requirements: RequirementSet,
    pub determinism: Determinism,
    /// Determinism contributed by this body apart from unresolved callable parameters.
    /// None means the independent contribution is not available from checked facts.
    #[serde(default)]
    pub independent_determinism: Option<Determinism>,
    pub support: InterpreterSupport,
}

impl Default for EffectSummary {
    fn default() -> Self {
        Self {
            handler_requests: Default::default(),
            default_handler_effects: None,
            exits: None,
            escaping_effects: Default::default(),
            requested_actions: Default::default(),
            default_actions: Default::default(),
            handled_actions: Default::default(),
            action_trace: Default::default(),
            residual_checks: Default::default(),
            trace_spec_obligations: Default::default(),
            requirements: Default::default(),
            determinism: Default::default(),
            independent_determinism: Some(Determinism::default()),
            support: Default::default(),
        }
    }
}

impl EffectSummary {
    pub fn local() -> Self {
        Self::default()
    }

    pub fn require_runtime(&mut self, reason: RuntimeRequirementReason) {
        self.determinism.join_assign(&Determinism::RuntimeMediated);
        if let Some(independent) = &mut self.independent_determinism {
            independent.join_assign(&Determinism::RuntimeMediated);
        }
        self.support.join_assign(&InterpreterSupport::from(reason));
    }

    pub fn record_escaping_effect(&mut self, effect: Effect) {
        let row = EffectRow::closed(EffectSet::one(effect));
        self.record_escaping_effect_row(row);
    }

    pub fn record_escaping_effect_row(&mut self, row: EffectRow) {
        if let Some(effects) = &mut self.default_handler_effects {
            effects.direct.union_assign(&row);
            self.refresh_escaping_effects();
        } else {
            self.escaping_effects.union_assign(&row);
        }
    }

    pub fn record_requested_action(&mut self, action: Effect) {
        if !action.is_action() {
            return;
        }
        self.handler_requests.record_untyped(action.clone());
        let row = EffectRow::closed(EffectSet::one(action));
        self.requested_actions.union_assign(&row);
    }

    pub fn record_default_handled_action(&mut self, action: Effect) {
        if !action.is_action() {
            return;
        }
        let row = EffectRow::closed(EffectSet::one(action));
        self.default_actions.union_assign(&row);
    }

    pub fn record_handled_action_row(&mut self, row: EffectRow) {
        self.handled_actions.union_assign(&row);
    }

    pub fn record_action_trace_event(
        &mut self,
        action: Effect,
        span: Span,
        source: ActionEventSource,
    ) {
        if !action.is_action() {
            return;
        }
        self.action_trace
            .seq_assign(ActionTraceDomain::Event(ActionEvent {
                action,
                span,
                source,
            }));
    }

    pub fn seq_assign(&mut self, other: &Self) -> bool {
        let mut changed = self.join_escaping_effects(other);
        changed |= self.handler_requests.union_assign(&other.handler_requests);
        changed |= self
            .requested_actions
            .union_assign(&other.requested_actions);
        changed |= self.default_actions.union_assign(&other.default_actions);
        changed |= self.handled_actions.union_assign(&other.handled_actions);
        changed |= self.action_trace.seq_assign(other.action_trace.clone());
        changed |= self.residual_checks.union_assign(&other.residual_checks);
        changed |= self
            .trace_spec_obligations
            .union_assign(&other.trace_spec_obligations);
        changed |= self.requirements.union_assign(&other.requirements);
        changed |= self.determinism.join_assign(&other.determinism);
        changed |= join_independent_determinism(
            &mut self.independent_determinism,
            other.independent_determinism,
        );
        changed |= self.support.join_assign(&other.support);
        changed
    }

    pub fn join_branch(&mut self, other: &Self) -> bool {
        let exits_changed = match (&mut self.exits, &other.exits) {
            (Some(current), Some(other)) => current.join_assign(other),
            (None, Some(other)) => {
                self.exits = Some(other.clone());
                true
            }
            _ => false,
        };
        let mut changed = self.join_escaping_effects(other);
        changed |= self.handler_requests.union_assign(&other.handler_requests);
        changed |= exits_changed;
        changed |= self
            .requested_actions
            .union_assign(&other.requested_actions);
        changed |= self.default_actions.union_assign(&other.default_actions);
        changed |= self.handled_actions.union_assign(&other.handled_actions);
        changed |= self.action_trace.choice_assign(other.action_trace.clone());
        changed |= self.residual_checks.union_assign(&other.residual_checks);
        changed |= self
            .trace_spec_obligations
            .union_assign(&other.trace_spec_obligations);
        changed |= self.requirements.union_assign(&other.requirements);
        changed |= self.determinism.join_assign(&other.determinism);
        changed |= join_independent_determinism(
            &mut self.independent_determinism,
            other.independent_determinism,
        );
        changed |= self.support.join_assign(&other.support);
        changed
    }

    pub fn repeated(mut self) -> Self {
        self.action_trace = self.action_trace.repeat();
        self
    }

    pub fn stabilize_for_fixpoint(&mut self, current: Option<&Self>) {
        if let Some(exits) = &mut self.exits {
            exits.stabilize(current.and_then(|current| current.exits.as_ref()));
        }
        self.action_trace = self
            .action_trace
            .clone()
            .stabilize_for_fixpoint(current.map(|summary| &summary.action_trace));
    }

    pub fn join_summary(&mut self, other: &Self) -> bool {
        if self == &Self::default() {
            let changed = self != other;
            *self = other.clone();
            return changed;
        }
        if other == &Self::default() {
            return false;
        }
        self.join_branch(other)
    }

    pub fn merge_request_footprint_without_escaping_or_trace(&mut self, other: &Self) -> bool {
        let mut default_changed = false;
        // Escaping effects and trace placement belong to the enclosing checked
        // transfer. Do not reintroduce effects removed by its local handlers.
        if let Some(defaults) = &other.default_handler_effects {
            for handler in &defaults.handlers {
                default_changed |= self.default_handler_effects.is_none();
                let effects = self.default_handler_effects.get_or_insert_with(|| {
                    super::DefaultHandlerEffects {
                        direct: self.escaping_effects.clone(),
                        handlers: vec![],
                    }
                });
                default_changed |=
                    effects.union_handler(handler.action.clone(), &EffectRow::empty());
            }
        }
        let mut changed = self
            .requested_actions
            .union_assign(&other.requested_actions);
        changed |= self.handler_requests.union_assign(&other.handler_requests);
        changed |= default_changed;
        changed |= self.default_actions.union_assign(&other.default_actions);
        changed |= self.handled_actions.union_assign(&other.handled_actions);
        changed |= self.residual_checks.union_assign(&other.residual_checks);
        changed |= self
            .trace_spec_obligations
            .union_assign(&other.trace_spec_obligations);
        changed |= self.requirements.union_assign(&other.requirements);
        changed |= self.determinism.join_assign(&other.determinism);
        changed |= join_independent_determinism(
            &mut self.independent_determinism,
            other.independent_determinism,
        );
        changed |= self.support.join_assign(&other.support);
        changed
    }
}

fn join_independent_determinism(
    left: &mut Option<Determinism>,
    right: Option<Determinism>,
) -> bool {
    match (left.as_mut(), right) {
        (Some(left), Some(right)) => left.join_assign(&right),
        (Some(_), None) => {
            *left = None;
            true
        }
        (None, _) => false,
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum ActionTraceDomain {
    #[default]
    Empty,
    Event(ActionEvent),
    ParameterCall {
        target: ParameterCallTarget,
        parameter: String,
        span: Span,
    },
    Seq(Vec<ActionTraceDomain>),
    Choice(Vec<ActionTraceDomain>),
    Repeat(Box<ActionTraceDomain>),
    UnknownOrder(EffectSet),
    Widened {
        actions: EffectSet,
        parameter_calls: Vec<ParameterCallRef>,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ParameterCallRef {
    pub target: ParameterCallTarget,
    pub parameter: String,
    pub span: Span,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum ParameterCallTarget {
    Native(etas_hir::SymbolId),
    Interface(u32),
    Graph {
        graph: etas_code::CodeFingerprint,
        parameter: etas_package_metadata::CallableValueParameter,
    },
}

impl ActionTraceDomain {
    const FIXPOINT_TRACE_NODE_LIMIT: usize = 32;

    pub(crate) fn has_unordered_approximation(&self) -> bool {
        let mut pending = vec![self];
        while let Some(trace) = pending.pop() {
            match trace {
                Self::UnknownOrder(_) | Self::Widened { .. } => return true,
                Self::Seq(parts) | Self::Choice(parts) => pending.extend(parts),
                Self::Repeat(inner) => pending.push(inner),
                Self::Empty | Self::Event(_) | Self::ParameterCall { .. } => {}
            }
        }
        false
    }

    pub fn has_parameter_calls(&self) -> bool {
        let mut pending = vec![self];
        while let Some(trace) = pending.pop() {
            match trace {
                Self::ParameterCall { .. } => return true,
                Self::Widened {
                    parameter_calls, ..
                } if !parameter_calls.is_empty() => return true,
                Self::Seq(parts) | Self::Choice(parts) => pending.extend(parts),
                Self::Repeat(inner) => pending.push(inner),
                Self::Empty | Self::Event(_) | Self::UnknownOrder(_) | Self::Widened { .. } => {}
            }
        }
        false
    }

    pub fn seq_assign(&mut self, other: Self) -> bool {
        let joined = self.clone().seq(other);
        let changed = joined != *self;
        *self = joined;
        changed
    }

    pub fn choice_assign(&mut self, other: Self) -> bool {
        let joined = self.clone().choice(other);
        let changed = joined != *self;
        *self = joined;
        changed
    }

    pub fn repeat(self) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Repeat(inner) => Self::Repeat(inner),
            other => Self::Repeat(Box::new(other)),
        }
    }

    pub fn stabilize_for_fixpoint(self, current: Option<&Self>) -> Self {
        let needs_widening = self.node_count() > Self::FIXPOINT_TRACE_NODE_LIMIT
            || self.contains_repeat()
            || current.is_some_and(Self::contains_repeat);
        if !needs_widening {
            return self;
        }
        self.into_widened()
    }

    fn node_count(&self) -> usize {
        match self {
            Self::Empty
            | Self::Event(_)
            | Self::ParameterCall { .. }
            | Self::UnknownOrder(_)
            | Self::Widened { .. } => 1,
            Self::Seq(parts) | Self::Choice(parts) => {
                1 + parts.iter().map(Self::node_count).sum::<usize>()
            }
            Self::Repeat(inner) => 1 + inner.node_count(),
        }
    }

    fn contains_repeat(&self) -> bool {
        match self {
            Self::Repeat(_) => true,
            Self::Seq(parts) | Self::Choice(parts) => parts.iter().any(Self::contains_repeat),
            Self::Empty
            | Self::Event(_)
            | Self::ParameterCall { .. }
            | Self::UnknownOrder(_)
            | Self::Widened { .. } => false,
        }
    }

    pub fn action_set(&self) -> EffectSet {
        let mut actions = EffectSet::default();
        self.collect_actions(&mut actions);
        actions
    }

    fn collect_actions(&self, actions: &mut EffectSet) {
        match self {
            Self::Empty => {}
            Self::UnknownOrder(set) => {
                for action in set.iter() {
                    actions.insert(action.clone());
                }
            }
            Self::Widened { actions: set, .. } => {
                for action in set.iter() {
                    actions.insert(action.clone());
                }
            }
            Self::Event(event) => {
                actions.insert(event.action.clone());
            }
            Self::ParameterCall { .. } => {}
            Self::Seq(parts) | Self::Choice(parts) => {
                for part in parts {
                    part.collect_actions(actions);
                }
            }
            Self::Repeat(inner) => inner.collect_actions(actions),
        }
    }

    fn collect_parameter_calls(
        &self,
        calls: &mut BTreeMap<(ParameterCallTarget, u32, u32, u32), ParameterCallRef>,
    ) {
        match self {
            Self::ParameterCall {
                target,
                parameter,
                span,
            } => {
                calls.insert(
                    (*target, span.source.0, span.range.start.0, span.range.end.0),
                    ParameterCallRef {
                        target: *target,
                        parameter: parameter.clone(),
                        span: *span,
                    },
                );
            }
            Self::Widened {
                parameter_calls, ..
            } => {
                for call in parameter_calls {
                    calls.insert(
                        (
                            call.target,
                            call.span.source.0,
                            call.span.range.start.0,
                            call.span.range.end.0,
                        ),
                        call.clone(),
                    );
                }
            }
            Self::Seq(parts) | Self::Choice(parts) => {
                for part in parts {
                    part.collect_parameter_calls(calls);
                }
            }
            Self::Repeat(inner) => inner.collect_parameter_calls(calls),
            Self::Empty | Self::Event(_) | Self::UnknownOrder(_) => {}
        }
    }

    fn into_widened(self) -> Self {
        let actions = self.action_set();
        let mut calls = BTreeMap::new();
        self.collect_parameter_calls(&mut calls);
        if actions.is_empty() && calls.is_empty() {
            Self::Empty
        } else {
            Self::Widened {
                actions,
                parameter_calls: calls.into_values().collect(),
            }
        }
    }

    pub(crate) fn widened(self) -> Self {
        self.into_widened()
    }

    fn merge_widened(left: Self, right: Self) -> Self {
        let mut actions = left.action_set();
        actions.union_assign(&right.action_set());
        let mut calls = BTreeMap::new();
        left.collect_parameter_calls(&mut calls);
        right.collect_parameter_calls(&mut calls);
        if actions.is_empty() && calls.is_empty() {
            Self::Empty
        } else {
            Self::Widened {
                actions,
                parameter_calls: calls.into_values().collect(),
            }
        }
    }

    fn seq(self, other: Self) -> Self {
        let trace = match (self, other) {
            (Self::Empty, right) => right,
            (left, Self::Empty) => left,
            (Self::UnknownOrder(mut left), Self::UnknownOrder(right)) => {
                left.union_assign(&right);
                Self::UnknownOrder(left)
            }
            (left @ Self::Widened { .. }, right) | (right, left @ Self::Widened { .. }) => {
                Self::merge_widened(left, right)
            }
            (Self::UnknownOrder(mut left), right) | (right, Self::UnknownOrder(mut left)) => {
                if matches!(right, Self::ParameterCall { .. } | Self::Widened { .. }) {
                    Self::merge_widened(Self::UnknownOrder(left), right)
                } else {
                    left.union_assign(&right.action_set());
                    Self::UnknownOrder(left)
                }
            }
            (Self::Seq(mut left), Self::Seq(right)) => {
                left.extend(right);
                Self::Seq(left)
            }
            (Self::Seq(mut left), right) => {
                left.push(right);
                Self::Seq(left)
            }
            (left, Self::Seq(mut right)) => {
                let mut items = vec![left];
                items.append(&mut right);
                Self::Seq(items)
            }
            (left, right) => Self::Seq(vec![left, right]),
        };
        trace.widen_if_needed()
    }

    fn choice(self, other: Self) -> Self {
        let trace = match (self, other) {
            (left, right) if left == right => left,
            (Self::UnknownOrder(mut left), Self::UnknownOrder(right)) => {
                left.union_assign(&right);
                Self::UnknownOrder(left)
            }
            (left @ Self::Widened { .. }, right) | (right, left @ Self::Widened { .. }) => {
                Self::merge_widened(left, right)
            }
            (Self::UnknownOrder(mut left), right) | (right, Self::UnknownOrder(mut left)) => {
                if matches!(right, Self::ParameterCall { .. } | Self::Widened { .. }) {
                    Self::merge_widened(Self::UnknownOrder(left), right)
                } else {
                    left.union_assign(&right.action_set());
                    Self::UnknownOrder(left)
                }
            }
            (Self::Choice(mut left), Self::Choice(right)) => {
                left.extend(right);
                left.sort_by_key(|trace| format!("{trace:?}"));
                left.dedup();
                Self::Choice(left)
            }
            (Self::Choice(mut left), right) => {
                left.push(right);
                left.sort_by_key(|trace| format!("{trace:?}"));
                left.dedup();
                Self::Choice(left)
            }
            (left, Self::Choice(mut right)) => {
                let mut items = vec![left];
                items.append(&mut right);
                items.sort_by_key(|trace| format!("{trace:?}"));
                items.dedup();
                Self::Choice(items)
            }
            (left, right) => {
                let mut items = vec![left, right];
                items.sort_by_key(|trace| format!("{trace:?}"));
                items.dedup();
                Self::Choice(items)
            }
        };
        trace.widen_if_needed()
    }

    fn widen_if_needed(self) -> Self {
        if self.node_count() <= Self::FIXPOINT_TRACE_NODE_LIMIT && !self.contains_repeat() {
            return self;
        }
        self.into_widened()
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ActionEvent {
    pub action: Effect,
    pub span: Span,
    pub source: ActionEventSource,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ActionEventSource {
    Perform,
    StdIntrinsic,
    AgentCall,
    ExternalMetadata,
    Transfer,
    Unknown,
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct ResidualCheckSet {
    pub checks: BTreeSet<String>,
}

impl ResidualCheckSet {
    pub fn union_assign(&mut self, other: &Self) -> bool {
        let len = self.checks.len();
        self.checks.extend(other.checks.iter().cloned());
        self.checks.len() != len
    }
}

impl PartialOrder for EffectSummary {
    fn less_equal(&self, other: &Self) -> bool {
        (self.exits.is_none() || self.exits == other.exits)
            && self.handler_requests.less_equal(&other.handler_requests)
            && row_less_equal(&self.escaping_effects, &other.escaping_effects)
            && row_less_equal(
                self.direct_escaping_effects(),
                other.direct_escaping_effects(),
            )
            && self.default_handler_effects.as_ref().is_none_or(|effects| {
                effects.handlers.iter().all(|handler| {
                    other.default_handler_effects.as_ref().is_some_and(|other| {
                        other.handlers.iter().any(|candidate| {
                            candidate.action == handler.action
                                && row_less_equal(&handler.produced, &candidate.produced)
                        })
                    })
                })
            })
            && row_less_equal(&self.requested_actions, &other.requested_actions)
            && row_less_equal(&self.default_actions, &other.default_actions)
            && row_less_equal(&self.handled_actions, &other.handled_actions)
            && self.action_trace.less_equal(&other.action_trace)
            && self.residual_checks.less_equal(&other.residual_checks)
            && requirement_set_less_equal(
                &self.trace_spec_obligations,
                &other.trace_spec_obligations,
            )
            && requirement_set_less_equal(&self.requirements, &other.requirements)
            && self.determinism <= other.determinism
            && match (self.independent_determinism, other.independent_determinism) {
                (Some(left), Some(right)) => left <= right,
                (_, None) => true,
                (None, Some(_)) => false,
            }
            && self.support.less_equal(&other.support)
    }
}

fn row_less_equal(left: &EffectRow, right: &EffectRow) -> bool {
    left.effects
        .iter()
        .all(|effect| right.effects.contains(effect))
        && match (left.open, right.open) {
            (None, Some(_)) => true,
            (None, None) => true,
            (Some(left), Some(right)) => left == right,
            (Some(_), None) => false,
        }
}

fn requirement_set_less_equal(left: &RequirementSet, right: &RequirementSet) -> bool {
    left.iter().all(|fact| right.contains(fact))
}

impl ActionTraceDomain {
    fn less_equal(&self, other: &Self) -> bool {
        self == other || matches!(self, Self::Empty)
    }
}

impl ResidualCheckSet {
    fn less_equal(&self, other: &Self) -> bool {
        self.checks.is_subset(&other.checks)
    }
}

impl JoinSemiLattice for EffectSummary {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        self.join_summary(other)
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum Determinism {
    #[default]
    Deterministic,
    RuntimeMediated,
    NonDeterministic,
}

impl Determinism {
    pub fn join_assign(&mut self, other: &Self) -> bool {
        let joined = (*self).max(*other);
        let changed = joined != *self;
        *self = joined;
        changed
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum InterpreterSupport {
    #[default]
    LocalOnly,
    RequiresHost(HostRequirementSet),
    RequiresInterpreterOrchestration(InterpreterOrchestrationRequirementSet),
    Rejected(FrontendRejectionReason),
}

impl From<RuntimeRequirementReason> for InterpreterSupport {
    fn from(reason: RuntimeRequirementReason) -> Self {
        match reason {
            RuntimeRequirementReason::Agentic => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Agentic))
            }
            RuntimeRequirementReason::ToolCall => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::ToolCall))
            }
            RuntimeRequirementReason::HostAuthority => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::HostAuthority))
            }
            RuntimeRequirementReason::DurableMemory => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::DurableMemory))
            }
            RuntimeRequirementReason::Approval => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Approval))
            }
            RuntimeRequirementReason::Checkpoint => Self::RequiresInterpreterOrchestration(
                InterpreterOrchestrationRequirementSet::feature(InterpreterFeatureKind::Checkpoint),
            ),
            RuntimeRequirementReason::Time => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Time))
            }
            RuntimeRequirementReason::Network => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Network))
            }
            RuntimeRequirementReason::Tcp => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Tcp))
            }
            RuntimeRequirementReason::Stream => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Stream))
            }
            RuntimeRequirementReason::Tls => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Tls))
            }
            RuntimeRequirementReason::Browser => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Browser))
            }
            RuntimeRequirementReason::Console => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Console))
            }
            RuntimeRequirementReason::FileIO => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::FileIO))
            }
            RuntimeRequirementReason::Command => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Command))
            }
            RuntimeRequirementReason::SecretAccess => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::SecretAccess))
            }
            RuntimeRequirementReason::Compiler => {
                Self::RequiresHost(HostRequirementSet::one(HostRequirementKind::Compiler))
            }
            RuntimeRequirementReason::RuntimeHandler => Self::RequiresInterpreterOrchestration(
                InterpreterOrchestrationRequirementSet::feature(
                    InterpreterFeatureKind::EffectHandler,
                ),
            ),
        }
    }
}

impl RuntimeRequirementReason {
    pub fn host_requirement_kind(&self) -> Option<HostRequirementKind> {
        match self {
            Self::Agentic => Some(HostRequirementKind::Agentic),
            Self::ToolCall => Some(HostRequirementKind::ToolCall),
            Self::HostAuthority => Some(HostRequirementKind::HostAuthority),
            Self::DurableMemory => Some(HostRequirementKind::DurableMemory),
            Self::Approval => Some(HostRequirementKind::Approval),
            Self::Time => Some(HostRequirementKind::Time),
            Self::Network => Some(HostRequirementKind::Network),
            Self::Tcp => Some(HostRequirementKind::Tcp),
            Self::Stream => Some(HostRequirementKind::Stream),
            Self::Tls => Some(HostRequirementKind::Tls),
            Self::Browser => Some(HostRequirementKind::Browser),
            Self::Console => Some(HostRequirementKind::Console),
            Self::FileIO => Some(HostRequirementKind::FileIO),
            Self::Command => Some(HostRequirementKind::Command),
            Self::SecretAccess => Some(HostRequirementKind::SecretAccess),
            Self::Compiler => Some(HostRequirementKind::Compiler),
            Self::Checkpoint | Self::RuntimeHandler => None,
        }
    }
}

impl InterpreterSupport {
    pub(crate) fn less_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (left, right) if left == right => true,
            (Self::LocalOnly, _) => true,
            (_, Self::Rejected(_)) => true,
            (Self::Rejected(_), _) => false,
            (Self::RequiresHost(left), Self::RequiresHost(right)) => {
                left.kinds.is_subset(&right.kinds)
            }
            (Self::RequiresHost(left), Self::RequiresInterpreterOrchestration(right)) => {
                left.kinds.is_subset(&right.host.kinds)
            }
            (
                Self::RequiresInterpreterOrchestration(left),
                Self::RequiresInterpreterOrchestration(right),
            ) => {
                left.host.kinds.is_subset(&right.host.kinds)
                    && left.features.kinds.is_subset(&right.features.kinds)
            }
            (Self::RequiresInterpreterOrchestration(_), Self::RequiresHost(_)) => false,
            (Self::RequiresHost(_), Self::LocalOnly)
            | (Self::RequiresInterpreterOrchestration(_), Self::LocalOnly) => false,
        }
    }

    pub fn without_host_requirements(&self, removed: &[HostRequirementKind]) -> Self {
        match self {
            Self::RequiresHost(host) => {
                let mut host = host.clone();
                for kind in removed {
                    host.kinds.remove(kind);
                }
                if host.kinds.is_empty() {
                    Self::LocalOnly
                } else {
                    Self::RequiresHost(host)
                }
            }
            Self::RequiresInterpreterOrchestration(requirements) => {
                let mut requirements = requirements.clone();
                for kind in removed {
                    requirements.host.kinds.remove(kind);
                }
                if requirements.host.kinds.is_empty() && requirements.features.kinds.is_empty() {
                    Self::LocalOnly
                } else {
                    Self::RequiresInterpreterOrchestration(requirements)
                }
            }
            Self::LocalOnly | Self::Rejected(_) => self.clone(),
        }
    }

    pub fn join_assign(&mut self, other: &Self) -> bool {
        let joined = match (&*self, other) {
            (Self::Rejected(reason), _) | (_, Self::Rejected(reason)) => {
                Self::Rejected(reason.clone())
            }
            (
                Self::RequiresInterpreterOrchestration(left),
                Self::RequiresInterpreterOrchestration(right),
            ) => Self::RequiresInterpreterOrchestration(left.joined(right)),
            (Self::RequiresInterpreterOrchestration(requirements), Self::RequiresHost(host))
            | (Self::RequiresHost(host), Self::RequiresInterpreterOrchestration(requirements)) => {
                Self::RequiresInterpreterOrchestration(requirements.with_host(host))
            }
            (Self::RequiresInterpreterOrchestration(requirements), Self::LocalOnly)
            | (Self::LocalOnly, Self::RequiresInterpreterOrchestration(requirements)) => {
                Self::RequiresInterpreterOrchestration(requirements.clone())
            }
            (Self::RequiresHost(left), Self::RequiresHost(right)) => {
                Self::RequiresHost(left.joined(right))
            }
            (Self::RequiresHost(host), Self::LocalOnly)
            | (Self::LocalOnly, Self::RequiresHost(host)) => Self::RequiresHost(host.clone()),
            (Self::LocalOnly, Self::LocalOnly) => Self::LocalOnly,
        };
        let changed = joined != *self;
        *self = joined;
        changed
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct HostRequirementSet {
    pub kinds: BTreeSet<HostRequirementKind>,
}

impl HostRequirementSet {
    pub fn one(kind: HostRequirementKind) -> Self {
        Self {
            kinds: BTreeSet::from([kind]),
        }
    }

    pub fn joined(&self, other: &Self) -> Self {
        let mut kinds = self.kinds.clone();
        kinds.extend(other.kinds.iter().cloned());
        Self { kinds }
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct InterpreterFeatureSet {
    pub kinds: BTreeSet<InterpreterFeatureKind>,
}

impl InterpreterFeatureSet {
    pub fn one(kind: InterpreterFeatureKind) -> Self {
        Self {
            kinds: BTreeSet::from([kind]),
        }
    }

    pub fn joined(&self, other: &Self) -> Self {
        let mut kinds = self.kinds.clone();
        kinds.extend(other.kinds.iter().cloned());
        Self { kinds }
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct InterpreterOrchestrationRequirementSet {
    pub host: HostRequirementSet,
    pub features: InterpreterFeatureSet,
}

impl InterpreterOrchestrationRequirementSet {
    pub fn feature(kind: InterpreterFeatureKind) -> Self {
        Self {
            host: HostRequirementSet::default(),
            features: InterpreterFeatureSet::one(kind),
        }
    }

    pub fn with_host(&self, host: &HostRequirementSet) -> Self {
        Self {
            host: self.host.joined(host),
            features: self.features.clone(),
        }
    }

    pub fn joined(&self, other: &Self) -> Self {
        Self {
            host: self.host.joined(&other.host),
            features: self.features.joined(&other.features),
        }
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum HostRequirementKind {
    Agentic,
    ToolCall,
    HostAuthority,
    DurableMemory,
    Approval,
    Checkpoint,
    Time,
    Network,
    Tcp,
    Stream,
    Tls,
    Browser,
    Console,
    FileIO,
    Command,
    SecretAccess,
    Compiler,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum InterpreterFeatureKind {
    EffectHandler,
    Resume,
    Retry,
    Checkpoint,
    WorkflowOrchestration,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum RuntimeRequirementReason {
    Agentic,
    ToolCall,
    HostAuthority,
    DurableMemory,
    Approval,
    Checkpoint,
    Time,
    Network,
    Tcp,
    Stream,
    Tls,
    Browser,
    Console,
    FileIO,
    Command,
    SecretAccess,
    Compiler,
    RuntimeHandler,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum FrontendRejectionReason {
    UnresolvedEffect,
    UnsupportedHandler,
    EscapedEffect,
    MissingRequirement,
}

#[cfg(test)]
mod tests {
    use etas_core::{SourceId, Span, TextSize};

    use super::ActionTraceDomain;

    #[test]
    fn widening_keeps_distinct_parameter_identities_with_the_same_name_and_span() {
        let call = |symbol| ActionTraceDomain::ParameterCall {
            target: super::ParameterCallTarget::Native(etas_hir::SymbolId(symbol)),
            parameter: "f".into(),
            span: Span::empty(SourceId(7), TextSize(11)),
        };
        let trace = ActionTraceDomain::Seq(vec![call(1), call(2), call(1)]).widened();
        let ActionTraceDomain::Widened {
            parameter_calls, ..
        } = trace
        else {
            panic!("widened parameter trace expected");
        };
        assert_eq!(parameter_calls.len(), 2);
        assert_ne!(parameter_calls[0].target, parameter_calls[1].target);
    }

    #[test]
    fn widening_preserves_parameter_call_placeholders() {
        let trace = ActionTraceDomain::Repeat(Box::new(ActionTraceDomain::ParameterCall {
            target: super::ParameterCallTarget::Native(etas_hir::SymbolId(3)),
            parameter: "f".to_owned(),
            span: Span::empty(SourceId(7), TextSize(11)),
        }));

        let widened = trace.stabilize_for_fixpoint(None);

        let ActionTraceDomain::Widened {
            actions,
            parameter_calls,
        } = widened
        else {
            panic!("repeated parameter call should widen");
        };
        assert!(actions.is_empty());
        assert_eq!(parameter_calls.len(), 1);
        assert_eq!(parameter_calls[0].parameter, "f");
    }
}
