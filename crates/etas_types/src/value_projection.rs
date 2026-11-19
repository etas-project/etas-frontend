//! Resolve a value position through the checked source type graph.
use std::collections::BTreeSet;

use crate::{Type, TypeFacts, TypeId, TypeStore, TypeSubstitutionEngine, TypeSubstitutionError};
use etas_code::descriptor::ValueProjectionStep as Step;

pub fn project(
    store: &TypeStore,
    facts: &TypeFacts,
    ty: TypeId,
    path: &[Step],
) -> Result<Option<TypeId>, TypeSubstitutionError> {
    project_with_lookup(store, facts, None, ty, path)
}

pub fn project_indexed(
    store: &TypeStore,
    facts: &TypeFacts,
    lookup: &crate::ty::TypeLookupIndex,
    ty: TypeId,
    path: &[Step],
) -> Result<Option<TypeId>, TypeSubstitutionError> {
    project_with_lookup(store, facts, Some(lookup), ty, path)
}

fn project_with_lookup(
    store: &TypeStore,
    facts: &TypeFacts,
    lookup: Option<&crate::ty::TypeLookupIndex>,
    mut ty: TypeId,
    path: &[Step],
) -> Result<Option<TypeId>, TypeSubstitutionError> {
    for step in path {
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(ty) {
                return Ok(None);
            }
            let record = match store.get(ty) {
                Some(Type::Nominal(_)) => true,
                Some(Type::Applied { constructor, .. }) => {
                    matches!(store.get(TypeId(constructor.0)), Some(Type::Nominal(_)))
                }
                _ => false,
            };
            if !record {
                break;
            }
            let representation = match lookup {
                Some(index) => {
                    crate::ty::applied_representation_in_store_indexed(store, index, ty)?
                }
                None => crate::applied_representation_in_store(store, ty)?,
            };
            let Some(representation) = representation else {
                return Ok(None);
            };
            ty = representation;
        }
        ty = match (step, store.get(ty)) {
            (Step::CallResult, Some(Type::Function(signature))) if signature.input.is_empty() => {
                signature.output
            }
            (Step::TupleElement(position), Some(Type::Tuple(elements))) => {
                match elements.get(*position as usize) {
                    Some(ty) => *ty,
                    None => return Ok(None),
                }
            }
            (Step::Field(name), Some(Type::Record(record))) => {
                match record.fields.iter().find(|field| field.name == *name) {
                    Some(field) => field.ty,
                    None => return Ok(None),
                }
            }
            (
                Step::SequenceElement,
                Some(
                    Type::Array(element)
                    | Type::List(element)
                    | Type::Set(element)
                    | Type::Slice(element),
                ),
            )
            | (Step::ListHead, Some(Type::List(element))) => *element,
            (Step::ListTail, Some(Type::List(_))) => ty,
            (Step::MapKey, Some(Type::Map { key, .. })) => *key,
            (Step::MapValue, Some(Type::Map { value, .. })) => *value,
            (
                Step::VariantPayload {
                    variant,
                    position: 0,
                },
                Some(Type::Option(element)),
            ) if variant == "Some" => *element,
            (
                Step::VariantPayload {
                    variant,
                    position: 0,
                },
                Some(Type::Result { ok, .. }),
            ) if variant == "Ok" => *ok,
            (
                Step::VariantPayload {
                    variant,
                    position: 0,
                },
                Some(Type::Result { err, .. }),
            ) if variant == "Err" => *err,
            (
                Step::VariantPayload { variant, position },
                Some(Type::Applied { constructor, args }),
            ) => {
                let base = TypeId(constructor.0);
                let Some(layout) = facts.enum_layouts.get(&base) else {
                    return Ok(None);
                };
                let Some(field) = layout
                    .variants
                    .iter()
                    .find(|item| item.name == *variant)
                    .and_then(|item| item.fields.get(*position as usize))
                else {
                    return Ok(None);
                };
                if args.len() != layout.type_params.len() {
                    return Err(TypeSubstitutionError::InvalidApplication(ty));
                }
                let substitutions = layout
                    .type_params
                    .iter()
                    .cloned()
                    .zip(args.iter().copied())
                    .collect();
                let canonical = crate::ty::canonical_parameters_for_application(store, base, args)?;
                let engine = match lookup {
                    Some(index) => {
                        TypeSubstitutionEngine::existing_indexed(store, index, &substitutions)
                    }
                    None => TypeSubstitutionEngine::existing(store, &substitutions),
                };
                engine
                    .with_canonical_parameters(&canonical)
                    .substitute(*field)?
            }
            (Step::VariantPayload { variant, position }, Some(Type::Enum(_))) => {
                let Some(field) = facts
                    .enum_layouts
                    .get(&ty)
                    .and_then(|layout| layout.variants.iter().find(|item| item.name == *variant))
                    .and_then(|item| item.fields.get(*position as usize))
                else {
                    return Ok(None);
                };
                *field
            }
            _ => return Ok(None),
        };
    }
    Ok(Some(ty))
}
