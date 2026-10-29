mod display;
mod enum_payload;
mod nominal;
mod normalize;
mod primitive;
mod representation;
mod row;
mod scheme;
mod store;
mod substitution;

pub(crate) use store::TypeBaseBuildError;
pub use store::{SharedTypeBase, TypeBaseId, TypeInterner, TypeLookupIndex, TypeStore};

pub use display::display_type;
pub use nominal::{EnumTypeRef, NamedTypeRef, NominalTypeRef};
pub use normalize::{
    applied_representation, nominal_representation_parts, record_fields_with_applied_params,
    type_contains_named_param,
};
pub use primitive::PrimitiveType;
pub use representation::{TypeRepresentationMaterializer, materialize_representations};
pub use row::{EffectArgRef, EffectRef, EffectRowRef};
pub use scheme::{TypeConstructorId, TypeScheme, TypeVarId};
pub use substitution::{
    TypeRelocationInput, TypeRelocationMaterializationInput, TypeSubstitutionBudget,
    TypeSubstitutionEngine, TypeSubstitutionError, substitute_effect_row_params,
    substitute_named_params, substitute_named_params_in_store, substitute_type_params,
};

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct TypeId(pub u32);

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum Type {
    Primitive(PrimitiveType),
    IntegerLiteral {
        text: String,
    },
    Var(TypeVarId),
    Array(TypeId),
    List(TypeId),
    Map {
        key: TypeId,
        value: TypeId,
    },
    Set(TypeId),
    Range {
        index: TypeId,
    },
    Slice(TypeId),
    Option(TypeId),
    Result {
        ok: TypeId,
        err: TypeId,
    },
    Record(RecordType),
    Tuple(Vec<TypeId>),
    Enum(EnumTypeRef),
    Function(FlowType),
    Handler(HandlerType),
    Named(NamedTypeRef),
    Nominal(NominalTypeRef),
    Applied {
        constructor: TypeConstructorId,
        args: Vec<TypeId>,
    },
    Refined {
        base: TypeId,
        predicate: RefinementId,
    },
    Trust {
        wrapper: TrustWrapper,
        inner: TypeId,
    },
    Schema(TypeId),
    Prompt,
    PromptPart,
    Message(TypeId),
    MemorySelection(TypeId),
    Store {
        key: TypeId,
        value: TypeId,
    },
    MemoryPlace(MemoryPlaceType),
    MemoryRegion(TypeId),
    ResourceHandle(ResourceHandleType),
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct RefinementId(pub u32);

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct RecordType {
    pub fields: Vec<FieldType>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct FieldType {
    pub name: String,
    pub ty: TypeId,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct FlowType {
    pub input: Vec<TypeId>,
    pub output: TypeId,
    pub effects: Option<EffectRowRef>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandlerType {
    pub handled: EffectRowRef,
    pub produced: HandlerProducedEffects,
    pub result: Option<TypeId>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum HandlerProducedEffects {
    Infer,
    Explicit(EffectRowRef),
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct MemoryPlaceType {
    pub segments: Vec<String>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum ResourceHandleType {
    MemoryRegion { schema: TypeId },
    ExternalTool { signature: TypeId },
    Other { name: String, args: Vec<TypeId> },
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum TrustWrapper {
    Trusted,
    Untrusted,
    Secret,
    Public,
    Sanitized,
}

impl std::fmt::Display for TrustWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Trusted => "Trusted",
            Self::Untrusted => "Untrusted",
            Self::Secret => "Secret",
            Self::Public => "Public",
            Self::Sanitized => "Sanitized",
        })
    }
}
