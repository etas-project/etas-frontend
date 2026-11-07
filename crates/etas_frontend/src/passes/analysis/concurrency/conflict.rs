use etas_effects::{
    ActionRef, Effect, EffectActionArgKind, EffectRegistry, MEMORY_READ_ACTION, MEMORY_TAG,
};
use etas_types::EffectArgRef;

use super::{BranchFact, ResourceConflict, ResourceGuardPlan, SchedulingDecision};

enum PairConflict {
    Disjoint,
    Known,
    Guard(ResourceConflict),
}

enum EffectConflict {
    Disjoint,
    Known,
    Guard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectorRelation {
    Disjoint,
    Overlap,
    Dynamic,
}

pub(super) fn scheduling_decision(
    branches: &[BranchFact],
    registry: &EffectRegistry,
    incomplete: bool,
) -> SchedulingDecision {
    if incomplete {
        return SchedulingDecision::Serialize;
    }
    let mut guards = Vec::new();
    for (index, left) in branches.iter().enumerate() {
        let repeated = left.shape == super::TaskBranchShape::MapperInvocation;
        for right in branches[index + 1..].iter().chain(repeated.then_some(left)) {
            match branch_conflict(left, right, registry) {
                PairConflict::Disjoint => {}
                PairConflict::Known => return SchedulingDecision::Serialize,
                PairConflict::Guard(guard) => guards.push(guard),
            }
        }
    }
    if guards.is_empty() {
        SchedulingDecision::Concurrent
    } else {
        SchedulingDecision::RuntimeGuard(ResourceGuardPlan { conflicts: guards })
    }
}

fn branch_conflict(
    left: &BranchFact,
    right: &BranchFact,
    registry: &EffectRegistry,
) -> PairConflict {
    let mut guard = None;
    for left_effect in left.actions.iter().chain(left.effects.effects.iter()) {
        for right_effect in right.actions.iter().chain(right.effects.effects.iter()) {
            match effect_conflict(left_effect, right_effect, registry) {
                EffectConflict::Disjoint => {}
                EffectConflict::Known => return PairConflict::Known,
                EffectConflict::Guard => {
                    guard.get_or_insert_with(|| ResourceConflict {
                        left_task: left.task_site,
                        right_task: right.task_site,
                        left: left_effect.clone(),
                        right: right_effect.clone(),
                    });
                }
            }
        }
    }
    guard.map_or(PairConflict::Disjoint, PairConflict::Guard)
}

fn effect_conflict(left: &Effect, right: &Effect, registry: &EffectRegistry) -> EffectConflict {
    match (left, right) {
        (Effect::Action(left), Effect::Action(right)) => {
            action_conflict(left, None, right, None, registry)
        }
        (Effect::Action(left), Effect::AppliedAction(right)) => {
            action_conflict(left, None, &right.action, Some(&right.args), registry)
        }
        (Effect::AppliedAction(left), Effect::Action(right)) => {
            action_conflict(&left.action, Some(&left.args), right, None, registry)
        }
        (Effect::AppliedAction(left), Effect::AppliedAction(right)) => action_conflict(
            &left.action,
            Some(&left.args),
            &right.action,
            Some(&right.args),
            registry,
        ),
        (Effect::Tag(left), Effect::Tag(right)) if left == right => EffectConflict::Known,
        (Effect::Tag(left), Effect::Applied { tag: right, .. })
        | (Effect::Applied { tag: left, .. }, Effect::Tag(right))
            if left == right =>
        {
            EffectConflict::Known
        }
        (
            Effect::Applied { tag: left, args },
            Effect::Applied {
                tag: right,
                args: other,
            },
        ) if left == right => {
            if args == other {
                EffectConflict::Known
            } else {
                EffectConflict::Guard
            }
        }
        (Effect::Error(left), Effect::Error(right)) if left == right => EffectConflict::Known,
        (Effect::Var(_), _) | (_, Effect::Var(_)) => EffectConflict::Known,
        _ => EffectConflict::Disjoint,
    }
}

fn action_conflict(
    left: &ActionRef,
    left_args: Option<&[EffectArgRef]>,
    right: &ActionRef,
    right_args: Option<&[EffectArgRef]>,
    registry: &EffectRegistry,
) -> EffectConflict {
    if left.tag != right.tag {
        return EffectConflict::Disjoint;
    }
    if left.tag == MEMORY_TAG
        && left.action == MEMORY_READ_ACTION
        && right.action == MEMORY_READ_ACTION
    {
        return EffectConflict::Disjoint;
    }
    let (Some(left_args), Some(right_args)) = (left_args, right_args) else {
        return EffectConflict::Known;
    };
    let Some(left_signature) = registry.action_signature(left) else {
        return EffectConflict::Known;
    };
    let Some(right_signature) = registry.action_signature(right) else {
        return EffectConflict::Known;
    };
    let left_resources = resource_selectors(&left_signature.effect_args, left_args);
    let right_resources = resource_selectors(&right_signature.effect_args, right_args);
    if left_resources.is_empty() || right_resources.is_empty() {
        return EffectConflict::Known;
    }
    if left_resources.len() != right_resources.len() {
        return EffectConflict::Known;
    }
    let mut dynamic = false;
    for (left, right) in left_resources.into_iter().zip(right_resources) {
        match selector_relation(left, right, registry) {
            SelectorRelation::Disjoint => return EffectConflict::Disjoint,
            SelectorRelation::Overlap => {}
            SelectorRelation::Dynamic => dynamic = true,
        }
    }
    if dynamic {
        EffectConflict::Guard
    } else {
        EffectConflict::Known
    }
}

fn resource_selectors<'a>(
    kinds: &[EffectActionArgKind],
    args: &'a [EffectArgRef],
) -> Vec<&'a EffectArgRef> {
    kinds
        .iter()
        .zip(args)
        .filter_map(|(kind, argument)| {
            (!matches!(kind, EffectActionArgKind::Type)).then_some(argument)
        })
        .collect()
}

fn selector_relation(
    left: &EffectArgRef,
    right: &EffectArgRef,
    registry: &EffectRegistry,
) -> SelectorRelation {
    match (left, right) {
        (EffectArgRef::Wildcard, _) | (_, EffectArgRef::Wildcard) => SelectorRelation::Dynamic,
        (EffectArgRef::Path(left), EffectArgRef::Path(right)) => {
            if left.starts_with(right) || right.starts_with(left) {
                SelectorRelation::Overlap
            } else {
                SelectorRelation::Disjoint
            }
        }
        (EffectArgRef::String(left), EffectArgRef::String(right))
        | (EffectArgRef::Int(left), EffectArgRef::Int(right)) => {
            if left == right {
                SelectorRelation::Overlap
            } else {
                SelectorRelation::Disjoint
            }
        }
        (EffectArgRef::Type(left), EffectArgRef::Type(right)) => {
            if left == right
                || registry.memory_place_contains(*left, *right)
                || registry.memory_place_contains(*right, *left)
            {
                SelectorRelation::Overlap
            } else if registry.memory_place(*left).is_some()
                && registry.memory_place(*right).is_some()
            {
                SelectorRelation::Disjoint
            } else {
                SelectorRelation::Dynamic
            }
        }
        _ => SelectorRelation::Dynamic,
    }
}

#[cfg(test)]
mod tests {
    use etas_effects::{EffectRow, EffectSet, EffectTagId};
    use etas_hir::HirExprId;
    use etas_types::TypeId;

    use super::*;

    fn branch(site: u32, resource: u32) -> BranchFact {
        BranchFact {
            id: super::super::TaskSiteId {
                source: etas_core::SourceId(0),
                start: site,
                end: site + 1,
            },
            source_index: site as usize,
            task_site: HirExprId(site),
            effects: EffectRow::empty(),
            behavior: etas_effects::EffectSummary::default(),
            actions: EffectSet::one(Effect::Applied {
                tag: EffectTagId(1024),
                args: vec![TypeId(resource)],
            }),
            shape: super::super::TaskBranchShape::ExplicitStart,
        }
    }

    #[test]
    fn dynamic_resource_pairs_publish_a_runtime_guard() {
        let decision = scheduling_decision(
            &[branch(1, 10), branch(2, 20)],
            &EffectRegistry::default(),
            false,
        );
        let SchedulingDecision::RuntimeGuard(plan) = decision else {
            panic!("expected runtime guard, got {decision:?}");
        };
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].left_task, HirExprId(1));
        assert_eq!(plan.conflicts[0].right_task, HirExprId(2));
    }

    #[test]
    fn identical_resource_pairs_serialize() {
        assert_eq!(
            scheduling_decision(
                &[branch(1, 10), branch(2, 10)],
                &EffectRegistry::default(),
                false,
            ),
            SchedulingDecision::Serialize
        );
    }
}
