use etas_hir::{HirItemId, SymbolId};

use crate::{CheckedSpecBound, EffectRowRef, TypeId};

pub type FlowSignature = CallableSignature;
pub type ToolSignature = CallableSignature;

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct AgentSignature {
    pub component: TypeId,
    pub definition_type: Option<TypeId>,
    pub default_entry: Option<std::sync::Arc<AgentEntrySignature>>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct AgentEntrySignature {
    pub method: Option<SymbolId>,
    pub callable_type: TypeId,
    pub callable: CallableSignature,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct AgentMethodSignature {
    pub private_to: Option<etas_hir::HirModuleId>,
    pub inherent: bool,
    pub agent: HirItemId,
    pub component: TypeId,
    pub symbol: SymbolId,
    pub body: HirItemId,
    pub implementation: Option<HirItemId>,
    pub name: String,
    pub callable_type: TypeId,
    pub callable: CallableSignature,
}

impl AgentSignature {
    pub fn callable(&self) -> Option<&CallableSignature> {
        self.default_entry.as_ref().map(|entry| &entry.callable)
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ItemSignature {
    Flow(FlowSignature),
    Agent(AgentSignature),
    Tool(ToolSignature),
    TopLevelLet(TopLevelLetSignature),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CallableSignature {
    #[serde(default)]
    pub generic_params: Vec<CallableGenericParam>,
    pub params: Vec<TypeId>,
    pub output: TypeId,
    pub effects: Option<EffectRowRef>,
    #[serde(default)]
    pub requested_actions: Option<EffectRowRef>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CallableGenericParam {
    pub kind: CallableGenericParamKind,
    pub name: String,
    pub subject: TypeId,
    pub bounds: Vec<CheckedSpecBound>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CallableGenericParamKind {
    Type,
    Effect,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TopLevelLetSignature {
    pub ty: TypeId,
}

pub type ItemSignatureFacts = std::collections::HashMap<HirItemId, ItemSignature>;

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CallableSymbolSignature {
    pub symbol: SymbolId,
    pub signature: CallableSignature,
}
