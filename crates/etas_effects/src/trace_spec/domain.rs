use etas_hir::HirItemId;
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::{ActionTraceDomain, EffectRow, EffectUnit};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceSpecSummary {
    pub unit: Option<EffectUnit>,
    pub owner: Option<HirItemId>,
    pub action_trace: ActionTraceDomain,
    pub requested_actions: EffectRow,
    pub incomplete: bool,
}

impl TraceSpecSummary {
    pub fn for_unit(unit: EffectUnit) -> Self {
        Self {
            unit: Some(unit),
            owner: unit.owner(),
            ..Self::default()
        }
    }

    pub fn seq_assign(&mut self, other: &Self) -> bool {
        let mut changed = self.action_trace.seq_assign(other.action_trace.clone());
        changed |= self
            .requested_actions
            .union_assign(&other.requested_actions);
        let old = self.incomplete;
        self.incomplete |= other.incomplete;
        changed |= old != self.incomplete;
        changed
    }

    pub fn join_branch(&mut self, other: &Self) -> bool {
        let mut changed = self.action_trace.choice_assign(other.action_trace.clone());
        changed |= self
            .requested_actions
            .union_assign(&other.requested_actions);
        let old = self.incomplete;
        self.incomplete |= other.incomplete;
        changed |= old != self.incomplete;
        changed
    }

    pub fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }
}

impl JoinSemiLattice for TraceSpecSummary {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        if self.is_bottom_like() {
            let changed = self != other;
            *self = other.clone();
            return changed;
        }
        if other.is_bottom_like() {
            return false;
        }
        let mut changed = false;
        changed |= join_option(&mut self.unit, other.unit);
        changed |= join_option(&mut self.owner, other.owner);
        changed |= self.join_branch(other);
        changed
    }
}

impl PartialOrder for TraceSpecSummary {
    fn less_equal(&self, other: &Self) -> bool {
        option_less_equal(&self.unit, &other.unit)
            && option_less_equal(&self.owner, &other.owner)
            && trace_less_equal(&self.action_trace, &other.action_trace)
            && row_less_equal(&self.requested_actions, &other.requested_actions)
            && (!self.incomplete || other.incomplete)
    }
}

impl TraceSpecSummary {
    fn is_bottom_like(&self) -> bool {
        self.unit.is_none()
            && self.owner.is_none()
            && self.action_trace == ActionTraceDomain::Empty
            && self.requested_actions.effects.is_empty()
            && !self.incomplete
    }
}

pub type TraceSpecState = TraceSpecSummary;

fn join_option<T>(slot: &mut Option<T>, other: Option<T>) -> bool
where
    T: Copy + Eq,
{
    match (*slot, other) {
        (None, Some(other)) => {
            *slot = Some(other);
            true
        }
        _ => false,
    }
}

fn option_less_equal<T>(left: &Option<T>, right: &Option<T>) -> bool
where
    T: Eq,
{
    left == right || left.is_none()
}

fn trace_less_equal(left: &ActionTraceDomain, right: &ActionTraceDomain) -> bool {
    left == right || matches!(left, ActionTraceDomain::Empty)
}

fn row_less_equal(left: &EffectRow, right: &EffectRow) -> bool {
    left.effects
        .iter()
        .all(|effect| right.effects.contains(effect))
        && match (left.open, right.open) {
            (None, Some(_)) | (None, None) => true,
            (Some(left), Some(right)) => left == right,
            (Some(_), None) => false,
        }
}
