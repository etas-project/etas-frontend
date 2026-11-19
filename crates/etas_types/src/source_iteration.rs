//! The one-step execution ABI of the ordinary source Iter declaration.
//! Collection traversal and cursor construction remain source algorithms.
use crate::{Type, TypeFacts, TypeId, TypeStore, TypeSubstitutionEngine, TypeSubstitutionError};

#[derive(Clone, Copy, Debug)]
pub struct CursorLayout {
    pub constructor: TypeId,
    pub element: TypeId,
    pub step: TypeId,
    pub native_option: bool,
}

pub fn checked_cursor(
    store: &TypeStore,
    facts: &TypeFacts,
    owner: TypeId,
) -> Result<Option<CursorLayout>, TypeSubstitutionError> {
    checked_cursor_with_lookup(store, facts, None, owner)
}

pub fn checked_cursor_indexed(
    store: &TypeStore,
    facts: &TypeFacts,
    lookup: &crate::ty::TypeLookupIndex,
    owner: TypeId,
) -> Result<Option<CursorLayout>, TypeSubstitutionError> {
    checked_cursor_with_lookup(store, facts, Some(lookup), owner)
}

fn checked_cursor_with_lookup(
    store: &TypeStore,
    facts: &TypeFacts,
    lookup: Option<&crate::ty::TypeLookupIndex>,
    owner: TypeId,
) -> Result<Option<CursorLayout>, TypeSubstitutionError> {
    let Some(Type::Applied { constructor, args }) = store.get(owner) else {
        return Ok(None);
    };
    let constructor = TypeId(constructor.0);
    if facts.known_std_types.source_iterator != Some(constructor) {
        return Ok(None);
    }
    let [element] = args.as_slice() else {
        return Err(TypeSubstitutionError::InvalidApplication(owner));
    };
    let layout = match lookup {
        Some(lookup) => crate::ty::applied_representation_in_store_indexed(store, lookup, owner)?,
        None => crate::applied_representation_in_store(store, owner)?,
    }
    .ok_or(TypeSubstitutionError::InvalidApplication(owner))?;
    let Some(Type::Record(record)) = store.get(layout) else {
        return Err(TypeSubstitutionError::InvalidApplication(owner));
    };
    let [next] = record.fields.as_slice() else {
        return Err(TypeSubstitutionError::InvalidApplication(owner));
    };
    let Some(Type::Function(signature)) = store.get(next.ty) else {
        return Err(TypeSubstitutionError::InvalidApplication(owner));
    };
    if next.name != "next"
        || !signature.input.is_empty()
        || !signature
            .effects
            .as_ref()
            .is_some_and(|row| row.effects.is_empty() && row.tail.is_none())
    {
        return Err(TypeSubstitutionError::InvalidApplication(owner));
    }
    let (pair, native_option) = match store.get(signature.output) {
        Some(Type::Option(pair)) => (*pair, false),
        Some(Type::Applied { constructor, args })
            if Some(TypeId(constructor.0)) == facts.known_std_types.option_enum =>
        {
            let base = TypeId(constructor.0);
            let layout = facts
                .enum_layouts
                .get(&base)
                .ok_or(TypeSubstitutionError::MissingEnumLayout(base))?;
            if args.len() != layout.type_params.len() || layout.variants.len() != 2 {
                return Err(TypeSubstitutionError::InvalidApplication(signature.output));
            }
            let some = layout
                .variants
                .iter()
                .find(|variant| variant.name == "Some" && variant.fields.len() == 1);
            let none = layout
                .variants
                .iter()
                .any(|variant| variant.name == "None" && variant.fields.is_empty());
            let Some(some) = some.filter(|_| none) else {
                return Err(TypeSubstitutionError::InvalidApplication(signature.output));
            };
            let substitutions = layout
                .type_params
                .iter()
                .cloned()
                .zip(args.iter().copied())
                .collect();
            let canonical = crate::ty::canonical_parameters_for_application(store, base, args)?;
            let engine = match lookup {
                Some(lookup) => {
                    TypeSubstitutionEngine::existing_indexed(store, lookup, &substitutions)
                }
                None => TypeSubstitutionEngine::existing(store, &substitutions),
            };
            let pair = engine
                .with_canonical_parameters(&canonical)
                .substitute(some.fields[0])?;
            (pair, true)
        }
        _ => return Err(TypeSubstitutionError::InvalidApplication(signature.output)),
    };
    if !matches!(store.get(pair), Some(Type::Tuple(fields))
        if fields.as_slice() == [*element, owner])
    {
        return Err(TypeSubstitutionError::InvalidApplication(signature.output));
    }
    Ok(Some(CursorLayout {
        constructor,
        element: *element,
        step: signature.output,
        native_option,
    }))
}
