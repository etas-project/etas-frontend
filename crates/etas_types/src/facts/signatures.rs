use etas_hir::{HirItemId, SymbolId};

use crate::{CheckedSpecBound, EffectRowRef, TypeId};

pub type FlowSignature = CallableSignature;
pub type AgentSignature = CallableSignature;
pub type ToolSignature = CallableSignature;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ItemSignature {
    Flow(FlowSignature),
    Agent(AgentSignature),
    Tool(ToolSignature),
    TopLevelLet(TopLevelLetSignature),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallableSignature {
    #[serde(default)]
    pub generic_params: Vec<CallableGenericParam>,
    pub params: Vec<TypeId>,
    pub output: TypeId,
    pub effects: Option<EffectRowRef>,
    #[serde(default)]
    pub requested_actions: Option<EffectRowRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallableGenericParam {
    pub name: String,
    pub subject: TypeId,
    pub bounds: Vec<CheckedSpecBound>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TopLevelLetSignature {
    pub ty: TypeId,
}

pub type ItemSignatureFacts = std::collections::HashMap<HirItemId, ItemSignature>;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallableSymbolSignature {
    pub symbol: SymbolId,
    pub signature: CallableSignature,
}
