#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AliasPrecisionConfig {
    pub constraint_model: AliasConstraintModel,
    pub flow_sensitivity: FlowSensitivity,
    pub field_sensitivity: FieldSensitivity,
    pub index_sensitivity: IndexSensitivity,
    pub allocation_sensitivity: AllocationSensitivity,
    pub context_sensitivity: ContextSensitivity,
    pub heap_model: HeapModel,
    pub update_policy: UpdatePolicy,
    pub max_alias_set_size: usize,
    pub max_projection_depth: usize,
    pub max_fixpoint_iterations: usize,
}

impl Default for AliasPrecisionConfig {
    fn default() -> Self {
        Self::balanced()
    }
}

impl AliasPrecisionConfig {
    pub fn fast() -> Self {
        Self {
            constraint_model: AliasConstraintModel::UnificationBased,
            flow_sensitivity: FlowSensitivity::FlowInsensitive,
            field_sensitivity: FieldSensitivity::Collapsed,
            index_sensitivity: IndexSensitivity::Collapsed,
            allocation_sensitivity: AllocationSensitivity::PerExpression,
            context_sensitivity: ContextSensitivity::ContextInsensitive,
            heap_model: HeapModel::AllocationSite,
            update_policy: UpdatePolicy::AlwaysWeak,
            max_alias_set_size: 8,
            max_projection_depth: 2,
            max_fixpoint_iterations: 64,
        }
    }

    pub fn balanced() -> Self {
        Self {
            constraint_model: AliasConstraintModel::Hybrid {
                pre_unify_locals: true,
                inclusion_for_resources: true,
                inclusion_for_public_api: true,
            },
            flow_sensitivity: FlowSensitivity::FlowSensitive,
            field_sensitivity: FieldSensitivity::NamedFields,
            index_sensitivity: IndexSensitivity::ConstantIndex,
            allocation_sensitivity: AllocationSensitivity::PerExpression,
            context_sensitivity: ContextSensitivity::ContextInsensitive,
            heap_model: HeapModel::AllocationSite,
            update_policy: UpdatePolicy::StrongWhenMustAlias,
            max_alias_set_size: 32,
            max_projection_depth: 4,
            max_fixpoint_iterations: 128,
        }
    }

    pub fn precise() -> Self {
        Self {
            constraint_model: AliasConstraintModel::InclusionBased,
            flow_sensitivity: FlowSensitivity::FlowSensitive,
            field_sensitivity: FieldSensitivity::FullProjection,
            index_sensitivity: IndexSensitivity::KeySensitive,
            allocation_sensitivity: AllocationSensitivity::PerCallSite,
            context_sensitivity: ContextSensitivity::CallString { k: 1 },
            heap_model: HeapModel::ResourceAware,
            update_policy: UpdatePolicy::StrongWhenMustAlias,
            max_alias_set_size: 128,
            max_projection_depth: 8,
            max_fixpoint_iterations: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasConstraintModel {
    /// Andersen-style subset constraints. More precise, more expensive.
    InclusionBased,
    /// Steensgaard-style union/equivalence constraints. Faster, less precise.
    UnificationBased,
    /// Fast pre-unification plus precise inclusion for selected boundaries.
    Hybrid {
        pre_unify_locals: bool,
        inclusion_for_resources: bool,
        inclusion_for_public_api: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowSensitivity {
    FlowInsensitive,
    FlowSensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldSensitivity {
    Collapsed,
    NamedFields,
    FullProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexSensitivity {
    Collapsed,
    ConstantIndex,
    KeySensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationSensitivity {
    PerType,
    PerExpression,
    PerCallSite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSensitivity {
    ContextInsensitive,
    CallString { k: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapModel {
    NoHeap,
    AllocationSite,
    ResourceAware,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatePolicy {
    AlwaysWeak,
    StrongWhenMustAlias,
}
