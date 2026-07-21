use crate::{
    EffectActionSignature, FlowSignature, TypeConstructorId, TypeId,
    facts::signatures::{AgentSignature, ToolSignature},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResourceHandleFact {
    MemoryRegion { schema: TypeId, stable_id: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnownStdTypes {
    pub index_error: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolTypeFact {
    Param {
        ty: TypeId,
    },
    Local {
        ty: TypeId,
        mutable: bool,
    },
    Field {
        ty: TypeId,
    },
    Value {
        ty: TypeId,
    },
    TopLevelLet {
        ty: TypeId,
        classification: etas_hir::TopLevelLetClassification,
    },
    Flow {
        signature: FlowSignature,
    },
    Agent {
        signature: AgentSignature,
    },
    Tool {
        signature: ToolSignature,
    },
    Type {
        constructor: TypeConstructorId,
    },
    TypeAlias {
        target: TypeId,
        params: Vec<String>,
    },
    NominalType {
        constructor: TypeConstructorId,
        params: Vec<String>,
        representation: Option<TypeId>,
    },
    Effect {
        symbol: etas_hir::SymbolId,
    },
    EffectAction {
        signature: EffectActionSignature,
    },
    Spec {
        symbol: etas_hir::SymbolId,
    },
    Error,
}
