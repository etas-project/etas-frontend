mod display;
mod nominal;
mod normalize;
mod primitive;
mod representation;
mod row;
mod scheme;
mod substitution;

use std::collections::HashMap;

pub use display::display_type;
pub use nominal::{EnumTypeRef, NamedTypeRef, NominalTypeRef};
pub use normalize::{
    applied_representation, nominal_representation_parts, record_fields_with_applied_params,
    type_contains_named_param,
};
pub use primitive::PrimitiveType;
pub(crate) use representation::materialize_representations;
pub use row::{EffectArgRef, EffectRef, EffectRowRef};
pub use scheme::{TypeConstructorId, TypeScheme, TypeVarId};
pub use substitution::{
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
)]
pub struct TypeId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct RefinementId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RecordType {
    pub fields: Vec<FieldType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FieldType {
    pub name: String,
    pub ty: TypeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FlowType {
    pub input: Vec<TypeId>,
    pub output: TypeId,
    pub effects: Option<EffectRowRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct HandlerType {
    pub handled: EffectRowRef,
    pub produced: HandlerProducedEffects,
    pub result: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HandlerProducedEffects {
    Infer,
    Explicit(EffectRowRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MemoryPlaceType {
    pub segments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ResourceHandleType {
    MemoryRegion { schema: TypeId },
    ExternalTool { signature: TypeId },
    Other { name: String, args: Vec<TypeId> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TypeStore {
    types: Vec<Type>,
}

impl TypeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty);
        id
    }

    pub fn get(&self, id: TypeId) -> Option<&Type> {
        self.types.get(id.0 as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TypeId, &Type)> {
        self.types
            .iter()
            .enumerate()
            .map(|(index, ty)| (TypeId(index as u32), ty))
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypeInterner {
    store: TypeStore,
    types: HashMap<Type, TypeId>,
    primitives: HashMap<PrimitiveType, TypeId>,
}

impl TypeInterner {
    /// Complete a predeclared nominal without changing its identity or its references.
    pub(crate) fn define_nominal(&mut self, id: TypeId, representation: Option<TypeId>) {
        let Type::Nominal(mut nominal) = self.store.types[id.0 as usize].clone() else {
            unreachable!("only nominal declarations can be completed")
        };
        self.types.remove(&Type::Nominal(nominal.clone()));
        nominal.representation = representation;
        let ty = Type::Nominal(nominal);
        self.store.types[id.0 as usize] = ty.clone();
        self.types.insert(ty, id);
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_store(store: TypeStore) -> Self {
        let mut types = HashMap::new();
        let mut primitives = HashMap::new();
        for (id, ty) in store.iter() {
            types.entry(ty.clone()).or_insert(id);
            if let Type::Primitive(primitive) = ty {
                primitives.entry(*primitive).or_insert(id);
            }
        }
        Self {
            store,
            types,
            primitives,
        }
    }

    pub fn primitive(&mut self, primitive: PrimitiveType) -> TypeId {
        if let Some(id) = self.primitives.get(&primitive).copied() {
            return id;
        }
        self.intern(Type::Primitive(primitive))
    }

    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(id) = self.types.get(&ty).copied() {
            return id;
        }
        let id = self.store.intern(ty.clone());
        if let Type::Primitive(primitive) = ty {
            self.primitives.insert(primitive, id);
            self.types.insert(Type::Primitive(primitive), id);
        } else {
            self.types.insert(ty, id);
        }
        id
    }

    pub fn store(&self) -> &TypeStore {
        &self.store
    }

    pub fn into_store(self) -> TypeStore {
        self.store
    }
}
