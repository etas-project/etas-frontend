use crate::{EffectRowRef, TypeId};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FieldProjectionFact {
    pub receiver: TypeId,
    pub field: String,
    pub output: TypeId,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeferredEffectRowObligation {
    pub param: String,
    pub source: etas_hir::HirExprId,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GenericInstantiationFact {
    pub type_bindings: Vec<(String, TypeId)>,
    pub effect_row_bindings: Vec<(String, EffectRowRef)>,
    pub deferred_effect_row_obligations: Vec<DeferredEffectRowObligation>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TryExprTypeFact {
    pub operand: etas_hir::HirExprId,
    pub value_type: TypeId,
    pub result_type: TypeId,
    pub target_error: Option<TypeId>,
}
