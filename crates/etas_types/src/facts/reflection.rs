use etas_hir::SymbolId;

use crate::TypeId;

/// An imported description is not a source method signature or executable evidence.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub enum ExternalAgentReflectionFact {
    Unavailable,
    Available {
        header: std::sync::Arc<etas_code::descriptor::AgentHeaderInfo>,
        graph: std::sync::Arc<etas_code::descriptor::DescriptorGraphInput>,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CheckedReflectionTarget {
    Agent {
        declaration: SymbolId,
        component: TypeId,
    },
    Callable {
        declaration: SymbolId,
    },
    CallableValue {
        binding: SymbolId,
        signature: TypeId,
    },
    ExternalCallable {
        callable: etas_code::CallableId,
        selected_spec: Option<etas_code::DefinitionKey>,
    },
    AgentDefaultEntry {
        declaration: SymbolId,
        component: TypeId,
    },
    AgentValue {
        binding: SymbolId,
        component: TypeId,
        method: Option<SymbolId>,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedReflectionFact {
    pub target: CheckedReflectionTarget,
    pub descriptor_type: TypeId,
    pub selected_spec: Option<crate::CheckedSpecBound>,
    pub access_module: Option<etas_hir::HirModuleId>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum DefinitionOperation {
    WithBody,
    Compile,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum DefinitionFamily {
    Agent,
    Callable,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum DefinitionOperationSelection {
    Intrinsic {
        intrinsic: u32,
        overload: usize,
    },
    Action {
        target: etas_std::StdSymbolId,
        overload: usize,
    },
}

/// A type-checked origin recipe, not the analysis of a future compiled body.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedDefinitionOperation {
    pub operation: DefinitionOperation,
    pub selection: DefinitionOperationSelection,
    pub input: etas_hir::HirExprId,
    pub family: DefinitionFamily,
    pub contract_type: TypeId,
    /// Key of the solved type/effect-row bindings in TypeFacts.
    pub instantiation: etas_hir::HirExprId,
}
