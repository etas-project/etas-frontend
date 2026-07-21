use super::{
    ERROR_TAG, Effect, EffectActionArgKind, EffectRow, EffectSet, EffectTagId,
    registry::EffectRegistry,
};
use etas_types::{Assignable, EffectArgRef};

pub struct EffectCoverage<'a> {
    pub registry: &'a EffectRegistry,
    pub types: &'a etas_types::TypeStore,
}

impl EffectCoverage<'_> {
    pub fn covers(&self, allowed: &Effect, actual: &Effect) -> bool {
        match (allowed, actual) {
            (Effect::Tag(allowed), Effect::Tag(actual)) => self.tag_covers(*allowed, *actual),
            (Effect::Action(allowed), Effect::Action(actual)) => allowed == actual,
            (Effect::Action(allowed), Effect::AppliedAction(actual)) => allowed == &actual.action,
            (Effect::Tag(allowed), Effect::Action(actual)) => self.tag_covers(*allowed, actual.tag),
            (Effect::Tag(allowed), Effect::AppliedAction(actual)) => {
                self.tag_covers(*allowed, actual.action.tag)
            }
            (Effect::Error(allowed), Effect::Error(actual)) => self.same_type(*allowed, *actual),
            (Effect::AppliedAction(allowed), Effect::AppliedAction(actual)) => {
                self.applied_action_covers(allowed, actual)
            }
            (
                Effect::Applied {
                    tag: allowed_tag,
                    args: allowed_args,
                },
                Effect::Applied {
                    tag: actual_tag,
                    args: actual_args,
                },
            ) => self.applied_covers(*allowed_tag, allowed_args, *actual_tag, actual_args),
            (Effect::Tag(allowed), Effect::Applied { tag: actual, args }) => {
                args.is_empty() && self.tag_covers(*allowed, *actual)
            }
            (
                Effect::Applied {
                    tag: allowed_tag,
                    args: allowed_args,
                },
                Effect::Error(actual),
            ) if *allowed_tag == ERROR_TAG && allowed_args.len() == 1 => {
                self.same_type(allowed_args[0], *actual)
            }
            (Effect::Error(allowed), Effect::Applied { tag, args })
                if *tag == ERROR_TAG && args.len() == 1 =>
            {
                self.same_type(*allowed, args[0])
            }
            _ => allowed == actual,
        }
    }

    fn applied_action_covers(
        &self,
        allowed: &super::ActionInstanceRef,
        actual: &super::ActionInstanceRef,
    ) -> bool {
        if allowed.action != actual.action || allowed.args.len() != actual.args.len() {
            return false;
        }
        let signature = self.registry.action_signature(&allowed.action);
        allowed
            .args
            .iter()
            .zip(&actual.args)
            .enumerate()
            .all(|(index, (allowed, actual))| {
                match signature.and_then(|signature| signature.effect_args.get(index)) {
                    Some(EffectActionArgKind::MemoryPlace) => {
                        self.memory_place_arg_contains(allowed, actual)
                    }
                    Some(EffectActionArgKind::StaticResourcePath { .. }) => {
                        self.static_resource_path_arg_covers(allowed, actual)
                    }
                    _ => self.effect_arg_covers(allowed, actual),
                }
            })
    }

    pub fn row_covers(&self, allowed: &EffectRow, actual: &EffectRow) -> bool {
        if allowed.open.is_some() {
            return true;
        }
        if actual.open.is_some() {
            return false;
        }
        actual.effects.iter().all(|effect| {
            allowed
                .effects
                .iter()
                .any(|candidate| self.covers(candidate, effect))
        })
    }

    pub fn row_uncovered_by(&self, allowed: &EffectRow, actual: &EffectRow) -> EffectRow {
        if allowed.open.is_some() {
            return EffectRow::empty();
        }
        let effects = actual
            .effects
            .iter()
            .filter(|effect| {
                !allowed
                    .effects
                    .iter()
                    .any(|candidate| self.covers(candidate, effect))
            })
            .cloned()
            .collect::<EffectSet>();
        EffectRow {
            effects,
            open: actual.open,
        }
    }

    pub fn subtract_handled(&self, body: &EffectRow, handled: &EffectRow) -> EffectRow {
        let remaining = body
            .effects
            .iter()
            .filter(|effect| {
                !handled
                    .effects
                    .iter()
                    .any(|candidate| self.covers(candidate, effect))
            })
            .cloned()
            .collect::<EffectSet>();
        EffectRow {
            effects: remaining,
            open: body.open,
        }
    }

    fn applied_covers(
        &self,
        allowed_tag: EffectTagId,
        allowed_args: &[etas_types::TypeId],
        actual_tag: EffectTagId,
        actual_args: &[etas_types::TypeId],
    ) -> bool {
        if !self.tag_covers(allowed_tag, actual_tag) || allowed_args.len() != actual_args.len() {
            return false;
        }
        match (allowed_args, actual_args) {
            ([allowed], [actual]) if self.registry.is_memory_tag(allowed_tag) => {
                allowed_tag == actual_tag && self.memory_place_contains(*allowed, *actual)
            }
            _ => allowed_args
                .iter()
                .zip(actual_args)
                .all(|(allowed, actual)| self.same_type(*allowed, *actual)),
        }
    }

    fn tag_covers(&self, allowed: EffectTagId, actual: EffectTagId) -> bool {
        allowed == actual || self.registry.tag_extends(actual, allowed)
    }

    fn same_type(&self, lhs: etas_types::TypeId, rhs: etas_types::TypeId) -> bool {
        etas_types::TypeRelation::new(self.types)
            .assignable(lhs, rhs)
            .is_ok()
    }

    fn memory_place_contains(&self, parent: etas_types::TypeId, child: etas_types::TypeId) -> bool {
        self.registry.memory_place_contains(parent, child)
    }

    fn memory_place_arg_contains(&self, allowed: &EffectArgRef, actual: &EffectArgRef) -> bool {
        if let (Some(allowed), Some(actual)) = (
            self.memory_place_arg_segments(allowed),
            self.memory_place_arg_segments(actual),
        ) {
            return path_arg_contains(&allowed, &actual);
        }
        match (allowed, actual) {
            (EffectArgRef::Wildcard, _) => true,
            (EffectArgRef::Type(allowed), EffectArgRef::Type(actual)) => {
                self.memory_place_contains(*allowed, *actual)
            }
            _ => self.effect_arg_covers(allowed, actual),
        }
    }

    fn effect_arg_covers(&self, allowed: &EffectArgRef, actual: &EffectArgRef) -> bool {
        match (allowed, actual) {
            (EffectArgRef::Wildcard, _) => true,
            (EffectArgRef::Type(allowed), EffectArgRef::Type(actual)) => {
                self.same_type(*allowed, *actual)
            }
            _ => allowed == actual,
        }
    }

    fn static_resource_path_arg_covers(
        &self,
        allowed: &EffectArgRef,
        actual: &EffectArgRef,
    ) -> bool {
        match (allowed, actual) {
            (EffectArgRef::Wildcard, _) => true,
            (EffectArgRef::Path(allowed), EffectArgRef::Path(actual)) => {
                path_arg_matches(allowed, actual)
            }
            _ => self.effect_arg_covers(allowed, actual),
        }
    }

    fn memory_place_arg_segments(&self, arg: &EffectArgRef) -> Option<Vec<String>> {
        match arg {
            EffectArgRef::Path(segments) => Some(segments.clone()),
            EffectArgRef::Type(ty) => self
                .registry
                .memory_place(*ty)
                .map(|place| place.segments.clone()),
            EffectArgRef::Wildcard | EffectArgRef::String(_) | EffectArgRef::Int(_) => None,
        }
    }
}

fn path_arg_contains(allowed: &[String], actual: &[String]) -> bool {
    actual.len() >= allowed.len()
        && actual
            .iter()
            .zip(allowed)
            .all(|(actual, allowed)| actual == allowed)
}

fn path_arg_matches(allowed: &[String], actual: &[String]) -> bool {
    !allowed.is_empty()
        && (allowed == actual
            || (actual.len() >= allowed.len()
                && actual
                    .iter()
                    .rev()
                    .zip(allowed.iter().rev())
                    .all(|(actual, allowed)| actual == allowed)))
}
