use etas_hir::{HirExprId, HirItemId, SymbolId};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedAgentInferenceFact {
    pub owner: HirItemId,
    pub agent: HirItemId,
    pub method: SymbolId,
    pub prompt: HirExprId,
    pub output: crate::TypeId,
    pub schema: crate::TypeId,
    pub action_symbol: SymbolId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedAgentControllerFact {
    pub context: crate::TypeId,
    pub step: CheckedAgentOperationFact,
    pub invoke: CheckedAgentOperationFact,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedAgentOperationFact {
    pub target: etas_std::StdSymbolId,
    pub signature: crate::CallableSignature,
}
