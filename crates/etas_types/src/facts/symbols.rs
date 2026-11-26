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
    pub result_enum: Option<TypeId>,
    pub option_enum: Option<TypeId>,
    pub source_iterator: Option<TypeId>,
    pub source_slice: Option<TypeId>,
    pub trust_wrappers: std::collections::BTreeMap<TypeId, crate::TrustWrapper>,
    pub storage_resources: std::collections::BTreeSet<TypeId>,
    pub execution: KnownExecutionTypes,
    pub directory_list_limit_error: Option<TypeId>,
    pub file_read_limit_error: Option<TypeId>,
    pub filesystem_io_error: Option<TypeId>,
    pub index_error: Option<TypeId>,
    pub budget_exceeded: Option<TypeId>,
    pub text_error: Option<TypeId>,
    pub runtime_types: std::collections::BTreeMap<etas_std::RuntimeTypeRole, TypeId>,
    pub runtime_values: std::collections::BTreeMap<etas_std::StdValueRole, TypeId>,
    pub json_value: Option<TypeId>,
    pub message: Option<TypeId>,
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

/// Identities for the six source declarations used at the task execution ABI.
/// Public records and their fields are still defined exclusively by Etas.
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
pub struct KnownExecutionTypes {
    pub task: Option<TypeId>,
    pub scheduler: Option<TypeId>,
    pub continuation: Option<TypeId>,
    pub task_handle: Option<TypeId>,
    pub scheduler_state: Option<TypeId>,
    pub continuation_state: Option<TypeId>,
}

impl KnownExecutionTypes {
    pub(crate) fn remap(&mut self, mut map: impl FnMut(TypeId) -> TypeId) {
        for ty in [
            &mut self.task,
            &mut self.scheduler,
            &mut self.continuation,
            &mut self.task_handle,
            &mut self.scheduler_state,
            &mut self.continuation_state,
        ] {
            *ty = ty.map(&mut map);
        }
    }

    pub(crate) fn merge_missing(&mut self, other: Self) {
        self.task = self.task.or(other.task);
        self.scheduler = self.scheduler.or(other.scheduler);
        self.continuation = self.continuation.or(other.continuation);
        self.task_handle = self.task_handle.or(other.task_handle);
        self.scheduler_state = self.scheduler_state.or(other.scheduler_state);
        self.continuation_state = self.continuation_state.or(other.continuation_state);
    }
}

impl KnownStdTypes {
    pub fn slice_element(&self, store: &crate::TypeStore, ty: TypeId) -> Option<TypeId> {
        match store.get(ty)? {
            crate::Type::Applied { constructor, args }
                if self.source_slice == Some(TypeId(constructor.0)) =>
            {
                match args.as_slice() {
                    [element] => Some(*element),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub fn runtime_value(&self, role: etas_std::StdValueRole) -> Option<TypeId> {
        self.runtime_values.get(&role).copied()
    }
    pub fn runtime_type(&self, role: etas_std::RuntimeTypeRole) -> Option<TypeId> {
        self.runtime_types.get(&role).copied()
    }

    pub fn runtime_role(
        &self,
        store: &crate::TypeStore,
        ty: TypeId,
    ) -> Option<etas_std::RuntimeTypeRole> {
        let constructor = match store.get(ty)? {
            crate::Type::Applied { constructor, .. } => TypeId(constructor.0),
            _ => ty,
        };
        self.runtime_types
            .iter()
            .find_map(|(role, ty)| (*ty == constructor).then_some(*role))
    }

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
