use etas_types::{EffectArgRef, TypeId};

use super::{EffectActionId, EffectTagId};

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ActionRef {
    pub tag: EffectTagId,
    pub action: EffectActionId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ActionInstanceRef {
    pub action: ActionRef,
    pub args: Vec<EffectArgRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectActionSig {
    pub id: EffectActionId,
    pub owner: EffectTagId,
    pub name: String,
    pub effect_args: Vec<EffectActionArgKind>,
    #[serde(default)]
    pub selector_param_names: Vec<String>,
    #[serde(default)]
    pub selector_defaults: Vec<Option<EffectArgRef>>,
    pub params: Vec<TypeId>,
    pub output: TypeId,
    pub returns_never: bool,
    pub runtime_requirement: Option<crate::RuntimeRequirementReason>,
    pub high_impact_ack: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectActionArgKind {
    Type,
    MemoryPlace,
    StaticResourcePath { ty: String },
    StringPattern,
}
