use std::collections::BTreeSet;

use etas_types::TypeId;

use super::{ActionInstanceRef, ActionRef, EffectTagId, EffectVarId};

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Effect {
    Tag(EffectTagId),
    Action(ActionRef),
    AppliedAction(ActionInstanceRef),
    Applied { tag: EffectTagId, args: Vec<TypeId> },
    Var(EffectVarId),
    Error(TypeId),
}

impl Effect {
    pub fn is_action(&self) -> bool {
        matches!(self, Self::Action(_) | Self::AppliedAction(_))
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct EffectSet {
    effects: BTreeSet<Effect>,
}

impl EffectSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn one(effect: Effect) -> Self {
        Self::from_iter([effect])
    }

    pub fn insert(&mut self, effect: Effect) -> bool {
        self.effects.insert(effect)
    }

    pub fn contains(&self, effect: &Effect) -> bool {
        self.effects.contains(effect)
    }

    pub fn remove(&mut self, effect: &Effect) -> bool {
        self.effects.remove(effect)
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Effect> {
        self.effects.iter()
    }

    pub fn union_assign(&mut self, other: &EffectSet) -> bool {
        let len = self.effects.len();
        self.effects.extend(other.effects.iter().cloned());
        self.effects.len() != len
    }
}

impl FromIterator<Effect> for EffectSet {
    fn from_iter<T: IntoIterator<Item = Effect>>(iter: T) -> Self {
        Self {
            effects: iter.into_iter().collect(),
        }
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct EffectRow {
    pub effects: EffectSet,
    pub open: Option<EffectVarId>,
}

impl EffectRow {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty() && self.open.is_none()
    }

    pub fn closed(effects: EffectSet) -> Self {
        Self {
            effects,
            open: None,
        }
    }

    pub fn union_assign(&mut self, other: &EffectRow) -> bool {
        let mut changed = self.effects.union_assign(&other.effects);
        if self.open.is_none() && other.open.is_some() {
            self.open = other.open;
            changed = true;
        }
        changed
    }

    pub fn remove_effect(&mut self, effect: &Effect) -> bool {
        self.effects.remove(effect)
    }
}

pub fn effect_var_id_from_name(name: &str) -> EffectVarId {
    let mut hash = 0x811c9dc5u32;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    EffectVarId(hash)
}
