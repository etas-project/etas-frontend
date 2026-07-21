use std::collections::HashMap;

use super::{
    FieldType, FlowType, HandlerProducedEffects, HandlerType, MemoryPlaceType, PrimitiveType,
    RecordType, ResourceHandleType, Type, TypeId, TypeInterner, TypeStore,
};

pub fn applied_representation(interner: &mut TypeInterner, ty: TypeId) -> Option<TypeId> {
    let (representation, substitutions) = nominal_representation_parts(interner.store(), ty)?;
    Some(substitute_named_params(
        interner,
        representation,
        &substitutions,
    ))
}

pub fn nominal_representation_parts(
    store: &TypeStore,
    ty: TypeId,
) -> Option<(TypeId, HashMap<String, TypeId>)> {
    match store.get(ty)? {
        Type::Nominal(nominal) => Some((nominal.representation?, HashMap::new())),
        Type::Applied { constructor, args } => {
            let Type::Nominal(nominal) = store.get(TypeId(constructor.0))? else {
                return None;
            };
            let representation = nominal.representation?;
            let substitutions = nominal
                .params
                .iter()
                .cloned()
                .zip(args.iter().copied())
                .collect();
            Some((representation, substitutions))
        }
        _ => None,
    }
}

pub fn record_fields_with_applied_params(store: &TypeStore, ty: TypeId) -> Option<RecordType> {
    match store.get(ty)?.clone() {
        Type::Record(record) => Some(record),
        Type::Nominal(_) | Type::Applied { .. } => {
            let (representation, substitutions) = nominal_representation_parts(store, ty)?;
            let mut record = record_fields_with_applied_params(store, representation)?;
            for field in &mut record.fields {
                field.ty = substitute_named_params_readonly(store, field.ty, &substitutions);
            }
            Some(record)
        }
        _ => None,
    }
}

pub fn substitute_named_params(
    interner: &mut TypeInterner,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> TypeId {
    if substitutions.is_empty() {
        return ty;
    }
    let Some(ty_data) = interner.store().get(ty).cloned() else {
        return ty;
    };
    match ty_data {
        Type::Named(name) => substitutions.get(&name.name).copied().unwrap_or(ty),
        Type::Array(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Array(inner))
        }
        Type::List(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::List(inner))
        }
        Type::Map { key, value } => {
            let key = substitute_named_params(interner, key, substitutions);
            let value = substitute_named_params(interner, value, substitutions);
            interner.intern(Type::Map { key, value })
        }
        Type::Set(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Set(inner))
        }
        Type::Range { index } => {
            let index = substitute_named_params(interner, index, substitutions);
            interner.intern(Type::Range { index })
        }
        Type::Slice(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Slice(inner))
        }
        Type::Option(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Option(inner))
        }
        Type::Result { ok, err } => {
            let ok = substitute_named_params(interner, ok, substitutions);
            let err = substitute_named_params(interner, err, substitutions);
            interner.intern(Type::Result { ok, err })
        }
        Type::Record(record) => {
            let fields = record
                .fields
                .into_iter()
                .map(|field| FieldType {
                    name: field.name,
                    ty: substitute_named_params(interner, field.ty, substitutions),
                })
                .collect();
            interner.intern(Type::Record(RecordType { fields }))
        }
        Type::Tuple(elements) => {
            let elements = elements
                .into_iter()
                .map(|element| substitute_named_params(interner, element, substitutions))
                .collect();
            interner.intern(Type::Tuple(elements))
        }
        Type::Function(flow) => {
            let input = flow
                .input
                .into_iter()
                .map(|input| substitute_named_params(interner, input, substitutions))
                .collect();
            let output = substitute_named_params(interner, flow.output, substitutions);
            interner.intern(Type::Function(FlowType {
                input,
                output,
                effects: flow.effects,
            }))
        }
        Type::Handler(handler) => {
            let result = handler
                .result
                .map(|result| substitute_named_params(interner, result, substitutions));
            interner.intern(Type::Handler(HandlerType {
                handled: handler.handled,
                produced: match handler.produced {
                    HandlerProducedEffects::Infer => HandlerProducedEffects::Infer,
                    HandlerProducedEffects::Explicit(row) => HandlerProducedEffects::Explicit(row),
                },
                result,
            }))
        }
        Type::Applied { constructor, args } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_named_params(interner, arg, substitutions))
                .collect();
            interner.intern(Type::Applied { constructor, args })
        }
        Type::Refined { base, predicate } => {
            let base = substitute_named_params(interner, base, substitutions);
            interner.intern(Type::Refined { base, predicate })
        }
        Type::Trust { wrapper, inner } => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Trust { wrapper, inner })
        }
        Type::Schema(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Schema(inner))
        }
        Type::Message(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::Message(inner))
        }
        Type::MemorySelection(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::MemorySelection(inner))
        }
        Type::Store { key, value } => {
            let key = substitute_named_params(interner, key, substitutions);
            let value = substitute_named_params(interner, value, substitutions);
            interner.intern(Type::Store { key, value })
        }
        Type::MemoryRegion(inner) => {
            let inner = substitute_named_params(interner, inner, substitutions);
            interner.intern(Type::MemoryRegion(inner))
        }
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
            let schema = substitute_named_params(interner, schema, substitutions);
            interner.intern(Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                schema,
            }))
        }
        Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
            let signature = substitute_named_params(interner, signature, substitutions);
            interner.intern(Type::ResourceHandle(ResourceHandleType::ExternalTool {
                signature,
            }))
        }
        Type::ResourceHandle(ResourceHandleType::Other { name, args }) => {
            let args = args
                .into_iter()
                .map(|arg| substitute_named_params(interner, arg, substitutions))
                .collect();
            interner.intern(Type::ResourceHandle(ResourceHandleType::Other {
                name,
                args,
            }))
        }
        Type::Primitive(PrimitiveType::Never)
        | Type::Primitive(_)
        | Type::IntegerLiteral { .. }
        | Type::Var(_)
        | Type::Enum(_)
        | Type::Nominal(_)
        | Type::Prompt
        | Type::PromptPart
        | Type::MemoryPlace(MemoryPlaceType { .. }) => ty,
    }
}

fn substitute_named_params_readonly(
    store: &TypeStore,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
) -> TypeId {
    match store.get(ty) {
        Some(Type::Named(name)) => substitutions.get(&name.name).copied().unwrap_or(ty),
        _ => ty,
    }
}
