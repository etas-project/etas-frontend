use std::collections::{HashMap, HashSet};

use super::{
    EffectArgRef, HandlerProducedEffects, RecordType, ResourceHandleType, Type, TypeId,
    TypeInterner, TypeStore, TypeSubstitutionError, substitute_named_params,
    substitute_named_params_in_store,
};

pub fn applied_representation(
    interner: &mut TypeInterner,
    ty: TypeId,
) -> Result<Option<TypeId>, TypeSubstitutionError> {
    let Some((representation, substitutions)) = nominal_representation_parts(interner.store(), ty)
    else {
        return Ok(None);
    };
    substitute_named_params(interner, representation, &substitutions).map(Some)
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

pub fn record_fields_with_applied_params(
    store: &TypeStore,
    ty: TypeId,
) -> Result<Option<RecordType>, TypeSubstitutionError> {
    let Some(ty_data) = store.get(ty).cloned() else {
        return Err(TypeSubstitutionError::MissingType(ty));
    };
    match ty_data {
        Type::Record(record) => Ok(Some(record)),
        Type::Nominal(_) | Type::Applied { .. } => {
            let Some((representation, substitutions)) = nominal_representation_parts(store, ty)
            else {
                return Ok(None);
            };
            let Some(mut record) = record_fields_with_applied_params(store, representation)? else {
                return Ok(None);
            };
            for field in &mut record.fields {
                field.ty = substitute_named_params_in_store(store, field.ty, &substitutions)?;
            }
            Ok(Some(record))
        }
        _ => Ok(None),
    }
}

pub fn type_contains_named_param(
    store: &TypeStore,
    ty: TypeId,
    name: &str,
) -> Result<bool, TypeSubstitutionError> {
    let mut pending = vec![ty];
    let mut visited = HashSet::new();
    while let Some(ty) = pending.pop() {
        if !visited.insert(ty) {
            continue;
        }
        match store
            .get(ty)
            .ok_or(TypeSubstitutionError::MissingType(ty))?
        {
            Type::Named(named) if named.name == name => return Ok(true),
            Type::Array(inner)
            | Type::List(inner)
            | Type::Set(inner)
            | Type::Range { index: inner }
            | Type::Slice(inner)
            | Type::Option(inner)
            | Type::Schema(inner)
            | Type::Message(inner)
            | Type::MemorySelection(inner)
            | Type::MemoryRegion(inner)
            | Type::Refined { base: inner, .. }
            | Type::Trust { inner, .. } => pending.push(*inner),
            Type::Map { key, value }
            | Type::Store { key, value }
            | Type::Result {
                ok: key,
                err: value,
            } => {
                pending.push(*key);
                pending.push(*value);
            }
            Type::Record(record) => pending.extend(record.fields.iter().map(|field| field.ty)),
            Type::Tuple(elements) | Type::Applied { args: elements, .. } => {
                pending.extend(elements.iter().copied());
            }
            Type::Function(flow) => {
                pending.extend(flow.input.iter().copied());
                pending.push(flow.output);
                if let Some(row) = &flow.effects {
                    push_effect_row_types(row, &mut pending);
                }
            }
            Type::Handler(handler) => {
                push_effect_row_types(&handler.handled, &mut pending);
                if let HandlerProducedEffects::Explicit(row) = &handler.produced {
                    push_effect_row_types(row, &mut pending);
                }
                pending.extend(handler.result);
            }
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                pending.push(*schema);
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                pending.push(*signature);
            }
            Type::ResourceHandle(ResourceHandleType::Other { args, .. }) => {
                pending.extend(args.iter().copied());
            }
            _ => {}
        }
    }
    Ok(false)
}

fn push_effect_row_types(row: &super::EffectRowRef, pending: &mut Vec<TypeId>) {
    pending.extend(row.effects.iter().flat_map(|effect| {
        effect.args.iter().filter_map(|arg| match arg {
            EffectArgRef::Type(ty) => Some(*ty),
            _ => None,
        })
    }));
}
