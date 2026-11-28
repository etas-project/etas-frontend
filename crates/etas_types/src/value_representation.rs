//! Minimum value-storage ABI used by checked source implementations.
//! These projections expose plain scalar/collection representation, not std
//! algorithms or authority-bearing objects. Nominal lookalikes gain no role.
use crate::{PrimitiveType as P, Type, TypeId, TypeStore};

/// The resource schema remains part of the checked type. The selected source
/// record only carries the live region's descriptive identity.
pub fn source_memory_region_parameter(store: &TypeStore, ty: TypeId) -> Option<TypeId> {
    let [schema] = standard_nominal_arguments(store, ty, "std.memory.MemoryRegion")? else {
        return None;
    };
    Some(*schema)
}

pub fn memory_region_schema(store: &TypeStore, ty: TypeId) -> Option<TypeId> {
    let parameter = |ty| match store.get(ty)? {
        Type::MemoryRegion(schema)
        | Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema }) => Some(*schema),
        _ => source_memory_region_parameter(store, ty),
    };
    let schema = parameter(ty)?;
    Some(parameter(schema).unwrap_or(schema))
}

pub fn source_memory_store_parameters(store: &TypeStore, ty: TypeId) -> Option<(TypeId, TypeId)> {
    let [key, value] = standard_nominal_arguments(store, ty, "std.memory.Store")? else {
        return None;
    };
    Some((*key, *value))
}

fn standard_nominal_arguments<'a>(
    store: &'a TypeStore,
    ty: TypeId,
    name: &str,
) -> Option<&'a [TypeId]> {
    let Type::Applied { constructor, args } = store.get(ty)? else {
        return None;
    };
    match store.get(TypeId(constructor.0))? {
        Type::Nominal(nominal) if nominal.name == name => Some(args),
        _ => None,
    }
}

/// Optional primitive payloads use the selected source Option constructor.
pub fn option_parameter(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    ty: TypeId,
) -> Option<TypeId> {
    match store.get(ty)? {
        Type::Option(inner) => Some(*inner),
        Type::Applied { constructor, args } if known.option_enum == Some(TypeId(constructor.0)) => {
            match args.as_slice() {
                [inner] => Some(*inner),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The result of `?` uses the selected std declaration. The legacy provider
/// retains its builtin representation; source enums keep their nominal owner.
pub fn result_parameters(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    ty: TypeId,
) -> Option<(TypeId, TypeId)> {
    match store.get(ty)? {
        Type::Result { ok, err } => Some((*ok, *err)),
        Type::Applied { constructor, args } if known.result_enum == Some(TypeId(constructor.0)) => {
            match args.as_slice() {
                [ok, err] => Some((*ok, *err)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Empty/cell constructor storage also serves native source-defined sequence
/// sums. Projecting the layout never makes two nominal owners assignable.
pub fn sequence_element(
    interner: &mut crate::TypeInterner,
    owner: TypeId,
) -> Result<Option<TypeId>, crate::TypeSubstitutionError> {
    if let Some(Type::List(element)) = interner.store().get(owner) {
        return Ok(Some(*element));
    }
    let Some(layout) = crate::applied_representation(interner, owner)? else {
        return Ok(None);
    };
    Ok(match interner.store().get(layout) {
        Some(Type::List(element)) => Some(*element),
        _ => None,
    })
}

pub fn sequence_element_in_store(
    store: &TypeStore,
    owner: TypeId,
) -> Result<Option<TypeId>, crate::TypeSubstitutionError> {
    if let Some(Type::List(element)) = store.get(owner) {
        return Ok(Some(*element));
    }
    let Some(layout) = crate::applied_representation_in_store(store, owner)? else {
        return Ok(None);
    };
    Ok(match store.get(layout) {
        Some(Type::List(element)) => Some(*element),
        _ => None,
    })
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueLength {
    String,
    Bytes,
    Array,
    List,
    Slice,
    Map,
    Set,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueProjection {
    NumericBits(P),
    NumericFamilyBits,
    CharBits,
    Length(ValueLength),
    Utf8,
    RangeStart,
    RangeEnd,
    RangeIncludeStart,
    RangeIncludeEnd,
}

impl ValueProjection {
    pub fn name(self) -> &'static str {
        match self {
            Self::NumericBits(_) | Self::NumericFamilyBits | Self::CharBits => "bits",
            Self::Length(_) => "length",
            Self::Utf8 => "utf8",
            Self::RangeStart => "start",
            Self::RangeEnd => "end",
            Self::RangeIncludeStart => "include_start",
            Self::RangeIncludeEnd => "include_end",
        }
    }

    pub fn output_type(self, store: &TypeStore, receiver: TypeId) -> Option<TypeId> {
        let primitive = match self {
            Self::NumericBits(_) | Self::NumericFamilyBits | Self::CharBits => P::U128,
            Self::Length(_) => P::USize,
            Self::Utf8 => P::Bytes,
            Self::RangeIncludeStart | Self::RangeIncludeEnd => P::Bool,
            Self::RangeStart | Self::RangeEnd => {
                let Type::Range { index } = store.get(receiver)? else {
                    return None;
                };
                return Some(*index);
            }
        };
        store
            .iter()
            .find_map(|(id, ty)| (ty == &Type::Primitive(primitive)).then_some(id))
    }
}

/// Field solving reads a frozen type store. Representation output primitives
/// must exist even in a minimal source library that never names them itself.
pub fn materialize_output_types(interner: &mut crate::TypeInterner) {
    for primitive in [P::U128, P::Bool, P::USize, P::Bytes] {
        interner.primitive(primitive);
    }
}

pub fn value_projection(
    store: &TypeStore,
    receiver: TypeId,
    field: &str,
) -> Option<ValueProjection> {
    let value = store.get(receiver)?;
    match (value, field) {
        (Type::Primitive(P::Char), "bits") => Some(ValueProjection::CharBits),
        (Type::Primitive(primitive), "bits") if numeric_bits(*primitive) => {
            Some(ValueProjection::NumericBits(*primitive))
        }
        (Type::Primitive(P::String), "utf8") => Some(ValueProjection::Utf8),
        (Type::Range { .. }, "start") => Some(ValueProjection::RangeStart),
        (Type::Range { .. }, "end") => Some(ValueProjection::RangeEnd),
        (Type::Range { .. }, "include_start") => Some(ValueProjection::RangeIncludeStart),
        (Type::Range { .. }, "include_end") => Some(ValueProjection::RangeIncludeEnd),
        (value, "length") => Some(ValueProjection::Length(match value {
            Type::Primitive(P::String) => ValueLength::String,
            Type::Primitive(P::Bytes) => ValueLength::Bytes,
            Type::Array(_) => ValueLength::Array,
            Type::List(_) => ValueLength::List,
            Type::Slice(_) => ValueLength::Slice,
            Type::Map { .. } => ValueLength::Map,
            Type::Set(_) => ValueLength::Set,
            _ => return None,
        })),
        _ => None,
    }
}

pub fn checked_value_projection(
    store: &TypeStore,
    facts: &crate::SpecFacts,
    receiver: TypeId,
    field: &str,
) -> Option<ValueProjection> {
    checked_value_projection_with(store, receiver, field, || facts)
}

pub(crate) fn checked_value_projection_with<F: std::borrow::Borrow<crate::SpecFacts>>(
    store: &TypeStore,
    receiver: TypeId,
    field: &str,
    facts: impl FnOnce() -> F,
) -> Option<ValueProjection> {
    value_projection(store, receiver, field).or_else(|| {
        (field == "bits"
            && crate::numeric_family::supports_family_with(
                store,
                receiver,
                crate::numeric_family::Family::Number,
                facts,
            ))
        .then_some(ValueProjection::NumericFamilyBits)
    })
}

pub fn range_fields(store: &TypeStore, receiver: TypeId) -> Option<crate::RecordType> {
    let Type::Range { index } = store.get(receiver)? else {
        return None;
    };
    let boolean = store
        .iter()
        .find_map(|(id, ty)| (ty == &Type::Primitive(P::Bool)).then_some(id))?;
    Some(crate::RecordType {
        fields: [
            ("start", *index),
            ("end", *index),
            ("include_start", boolean),
            ("include_end", boolean),
        ]
        .into_iter()
        .map(|(name, ty)| crate::FieldType {
            name: name.into(),
            ty,
        })
        .collect(),
    })
}

pub fn numeric_bits(primitive: P) -> bool {
    matches!(
        primitive,
        P::I8
            | P::I16
            | P::I32
            | P::I64
            | P::I128
            | P::ISize
            | P::U8
            | P::U16
            | P::U32
            | P::U64
            | P::U128
            | P::USize
            | P::F32
            | P::F64
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveInput {
    Bits,
    Scalars,
    Data,
}

impl PrimitiveInput {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bits => "bits",
            Self::Scalars => "scalars",
            Self::Data => "data",
        }
    }

    pub fn materialize(self, interner: &mut crate::TypeInterner) -> TypeId {
        match self {
            Self::Bits => interner.primitive(P::U128),
            Self::Scalars => {
                let item = interner.primitive(P::Char);
                interner.intern(Type::Array(item))
            }
            Self::Data => {
                let item = interner.primitive(P::U8);
                interner.intern(Type::Array(item))
            }
        }
    }

    pub fn matches(self, store: &TypeStore, ty: TypeId) -> bool {
        match (self, store.get(ty)) {
            (Self::Bits, Some(Type::Primitive(P::U128))) => true,
            (Self::Scalars, Some(Type::Array(inner))) => {
                store.get(*inner) == Some(&Type::Primitive(P::Char))
            }
            (Self::Data, Some(Type::Array(inner))) => {
                store.get(*inner) == Some(&Type::Primitive(P::U8))
            }
            _ => false,
        }
    }
}

pub fn primitive_input(primitive: P) -> Option<PrimitiveInput> {
    match primitive {
        P::Char => Some(PrimitiveInput::Bits),
        P::String => Some(PrimitiveInput::Scalars),
        P::Bytes => Some(PrimitiveInput::Data),
        primitive if numeric_bits(primitive) => Some(PrimitiveInput::Bits),
        _ => None,
    }
}

/// Raw representation construction is also available for a checked member of
/// the closed primitive Number family. Its concrete width comes from the
/// caller's type argument, not from a standard-library algorithm binding.
pub fn checked_primitive_input(
    store: &TypeStore,
    facts: &crate::SpecFacts,
    target: TypeId,
) -> Option<PrimitiveInput> {
    checked_primitive_input_with(store, target, || facts)
}

pub(crate) fn checked_primitive_input_with<F: std::borrow::Borrow<crate::SpecFacts>>(
    store: &TypeStore,
    target: TypeId,
    facts: impl FnOnce() -> F,
) -> Option<PrimitiveInput> {
    match store.get(target) {
        Some(Type::Primitive(kind)) => primitive_input(*kind),
        _ if crate::numeric_family::supports_family_with(
            store,
            target,
            crate::numeric_family::Family::Number,
            facts,
        ) =>
        {
            Some(PrimitiveInput::Bits)
        }
        _ => None,
    }
}
