use crate::{EffectRowRef, TypeId};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedStdCallFact {
    pub qualified_path: Vec<String>,
    pub intrinsic: u32,
    pub overload: usize,
    pub callable_type: TypeId,
    pub signature: crate::CallableSignature,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct FieldProjectionFact {
    pub receiver: TypeId,
    pub field: String,
    pub output: TypeId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct DeferredEffectRowObligation {
    pub param: String,
    pub source: etas_hir::HirExprId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct GenericInstantiationFact {
    pub type_bindings: Vec<(String, TypeId)>,
    pub effect_row_bindings: Vec<(String, EffectRowRef)>,
    pub deferred_effect_row_obligations: Vec<DeferredEffectRowObligation>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CheckedIndexKind {
    Sequence {
        base: TypeId,
        index: TypeId,
        output: TypeId,
    },
    MapLookup {
        key: TypeId,
        value: TypeId,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum CheckedSliceKind {
    Sequence {
        base: TypeId,
        start: TypeId,
        end: TypeId,
        output: TypeId,
    },
    Range {
        range: TypeId,
        start: TypeId,
        end: TypeId,
        output: TypeId,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct TryExprTypeFact {
    pub operand: etas_hir::HirExprId,
    pub value_type: TypeId,
    pub result_type: TypeId,
    pub target_error: Option<TypeId>,
}
