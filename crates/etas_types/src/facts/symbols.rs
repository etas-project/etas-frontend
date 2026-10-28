use crate::{
    EffectActionSignature, FlowSignature, TypeConstructorId, TypeId,
    facts::signatures::{AgentSignature, ToolSignature},
};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ResourceHandleFact {
    MemoryRegion { schema: TypeId, stable_id: String },
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct KnownStdTypes {
    pub index_error: Option<TypeId>,
    pub command_result: Option<TypeId>,
    pub priority_queue: Option<TypeId>,
    pub ordered_map: Option<TypeId>,
    pub ordered_set: Option<TypeId>,
    pub opaque_values: std::collections::BTreeSet<TypeId>,
    pub support_constraints: std::collections::BTreeSet<TypeId>,
    pub iterables: std::collections::BTreeMap<TypeId, StdIterableFact>,
    pub code: KnownCodeTypes,
    pub intrinsic_only_constructors: std::collections::BTreeSet<TypeId>,
    pub projected_fields: std::collections::BTreeMap<TypeId, Vec<crate::FieldType>>,
}

impl KnownStdTypes {
    pub fn projected_field_templates(
        &self,
        store: &crate::TypeStore,
        ty: TypeId,
    ) -> Option<&[crate::FieldType]> {
        let constructor = match store.get(ty)? {
            crate::Type::Applied { constructor, .. } => TypeId(constructor.0),
            _ => ty,
        };
        self.projected_fields.get(&constructor).map(Vec::as_slice)
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct KnownCodeTypes {
    pub agent_id: Option<TypeId>,
    pub type_info: Option<TypeId>,
    pub generic_owner_info: Option<TypeId>,
    pub tool_ref: Option<TypeId>,
    pub static_value_info: Option<TypeId>,
    pub trace_constraint_info: Option<TypeId>,
    pub agent_definition: Option<TypeId>,
    pub callable_definition: Option<TypeId>,
    pub syntax_block: Option<TypeId>,
    pub definition_id: Option<TypeId>,
    pub callable_id: Option<TypeId>,
    pub effect_row_info: Option<TypeId>,
    pub action_summary: Option<TypeId>,
    pub trace_summary: Option<TypeId>,
    pub determinism: Option<TypeId>,
    pub descriptor_detail: Option<TypeId>,
    pub trace_witness: Option<TypeId>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct StdIterableFact {
    pub arity: usize,
    pub item: StdIterableItemFact,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum StdIterableItemFact {
    Element { parameter: usize },
    Entry { key: usize, value: usize },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
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
        params: Vec<crate::CallableGenericParam>,
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
