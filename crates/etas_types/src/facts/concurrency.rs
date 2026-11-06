use etas_hir::{HirHandlerArmId, ScopeId, SymbolId};

use crate::{EffectRowRef, TypeId};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct ContinuationBindingFact {
    pub arm: HirHandlerArmId,
    pub symbol: SymbolId,
    pub owner_scope: ScopeId,
    pub action_output: TypeId,
    pub answer: Option<TypeId>,
    pub residual_effects: Option<EffectRowRef>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TaskOwnerFact {
    pub owner_scope: ScopeId,
    pub output: TypeId,
    pub effects: EffectRowRef,
}
