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
