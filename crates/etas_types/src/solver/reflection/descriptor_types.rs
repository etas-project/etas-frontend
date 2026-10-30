use std::collections::{BTreeMap, BTreeSet};

use crate::{PrimitiveType as N, ResourceHandleType, TrustWrapper, Type, TypeId};
use etas_code::descriptor::{
    BuiltinTypeInfo as B, DescriptorGraphInput, GenericParameterRef, PrimitiveInfo as P, TypeInfo,
    TypeInfoRef,
};

use super::{ReflectionSolveInput, resolve_known_substitutions};

struct TypeMatchInput<'a> {
    store: &'a crate::TypeStore,
    facts: &'a crate::TypeFacts,
    solved: Option<&'a crate::SolverReport>,
    native_binders: Option<&'a BTreeMap<GenericParameterRef, TypeId>>,
    native_names: Option<&'a BTreeMap<String, TypeId>>,
    native_effect_binders: Option<&'a BTreeMap<GenericParameterRef, String>>,
}

/// Compare identities and structural types, never nominal representations.
pub(super) fn matches(
    input: &ReflectionSolveInput<'_>,
    graph: &DescriptorGraphInput,
    actual: TypeId,
    expected: TypeInfoRef,
) -> Result<bool, &'static str> {
    matches_scoped(
        &TypeMatchInput {
            store: input.store,
            facts: input.facts,
            solved: Some(input.solved),
            native_binders: None,
            native_names: None,
            native_effect_binders: None,
        },
        graph,
        actual,
        expected,
        &BTreeMap::new(),
    )
}

/// Compare a finalized checked type with a validated portable descriptor.
pub fn checked_descriptor_type_matches(
    types: &crate::TypeOutput,
    graph: &etas_code::descriptor::DescriptorGraph,
    actual: TypeId,
    expected: TypeInfoRef,
) -> Result<bool, &'static str> {
    matches_scoped(
        &TypeMatchInput {
            store: &types.store,
            facts: &types.facts,
            solved: None,
            native_binders: None,
            native_names: None,
            native_effect_binders: None,
        },
        graph.data(),
        actual,
        expected,
        &BTreeMap::new(),
    )
}

/// Compare a checked type using call-site bindings for the owning descriptor's binders.
pub fn checked_descriptor_type_matches_with_bindings(
    types: &crate::TypeOutput,
    graph: &etas_code::descriptor::DescriptorGraph,
    actual: TypeId,
    expected: TypeInfoRef,
    binders: &BTreeMap<GenericParameterRef, TypeId>,
    names: &BTreeMap<String, TypeId>,
    effect_binders: &BTreeMap<GenericParameterRef, String>,
) -> Result<bool, &'static str> {
    matches_scoped(
        &TypeMatchInput {
            store: &types.store,
            facts: &types.facts,
            solved: None,
            native_binders: Some(binders),
            native_names: Some(names),
            native_effect_binders: Some(effect_binders),
        },
        graph.data(),
        actual,
        expected,
        &BTreeMap::new(),
    )
}

fn resolve_native_binding(
    input: &TypeMatchInput<'_>,
    mut ty: TypeId,
) -> Result<TypeId, &'static str> {
    let mut visited = BTreeSet::new();
    while let Some(Type::Named(named)) = input.store.get(ty) {
        if named.source_parameter.is_some() {
            break;
        }
        let Some(bound) = input
            .native_names
            .and_then(|names| names.get(&named.name))
            .copied()
        else {
            break;
        };
        if bound == ty {
            break;
        }
        if !visited.insert(ty) {
            return Err("cyclic checked generic type binding");
        }
        ty = bound;
    }
    Ok(ty)
}

fn matches_scoped(
    input: &TypeMatchInput<'_>,
    graph: &DescriptorGraphInput,
    actual: TypeId,
    expected: TypeInfoRef,
    environment: &BTreeMap<GenericParameterRef, TypeInfoRef>,
) -> Result<bool, &'static str> {
    let mut environments = vec![environment.clone()];
    let mut pending = vec![(actual, expected, 0usize)];
    let mut seen = BTreeSet::new();
    'work: while let Some((actual, mut expected, scope)) = pending.pop() {
        let environment = &environments[scope];
        let actual = input.solved.map_or(actual, |solved| {
            resolve_known_substitutions(input.store, solved, actual)
        });
        let actual = resolve_native_binding(input, actual)?;
        let mut parameters = BTreeSet::new();
        while let Some(TypeInfo::Parameter(parameter)) = graph.types.get(expected.0 as usize) {
            let Some(bound) = environment.get(parameter) else {
                let Some(native) = input
                    .native_binders
                    .and_then(|binders| binders.get(parameter))
                else {
                    return Ok(false);
                };
                if actual != resolve_native_binding(input, *native)? {
                    return Ok(false);
                }
                continue 'work;
            };
            if !parameters.insert(*parameter) {
                return Err("cyclic descriptor type parameter binding");
            }
            expected = *bound;
        }
        if !seen.insert((actual, expected, scope)) {
            continue;
        }
        let node = graph
            .types
            .get(expected.0 as usize)
            .ok_or("selected spec has a missing type descriptor")?;
        if let TypeInfo::Alias { target, .. } = node {
            pending.push((actual, *target, scope));
            continue;
        }
        if let TypeInfo::Applied {
            constructor,
            arguments,
        } = node
            && let Some(TypeInfo::Alias {
                parameters, target, ..
            }) = graph.types.get(constructor.0 as usize)
        {
            if parameters.len() != arguments.len() {
                return Err("selected spec has an inconsistent alias application");
            }
            let mut next = environment.clone();
            for (parameter, argument) in parameters.iter().zip(arguments) {
                next.insert(*parameter, *argument);
            }
            let scope = environments.len();
            environments.push(next);
            pending.push((actual, *target, scope));
            continue;
        }
        let ty = input
            .store
            .get(actual)
            .ok_or("selected spec argument has no checked type")?;
        let mut children = Vec::new();
        match (ty, node) {
            (Type::Primitive(native), TypeInfo::Primitive(described))
                if primitive(*native) == *described => {}
            (
                Type::Nominal(_) | Type::Enum(_) | Type::Named(_),
                TypeInfo::Nominal { definition, .. } | TypeInfo::Enum { definition, .. },
            ) => {
                if input.facts.reflection_type_identities.get(&actual) != Some(definition) {
                    return Ok(false);
                }
            }
            (Type::Tuple(actual), TypeInfo::Tuple(expected)) => {
                if actual.len() != expected.len() {
                    return Ok(false);
                }
                children.extend(actual.iter().copied().zip(expected.iter().copied()));
            }
            (Type::Record(actual), TypeInfo::Record(expected)) => {
                if actual.fields.len() != expected.len() {
                    return Ok(false);
                }
                for field in expected {
                    let Some(actual) = actual
                        .fields
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                    else {
                        return Ok(false);
                    };
                    children.push((actual.ty, field.ty));
                }
            }
            (
                Type::Applied { constructor, args },
                TypeInfo::Builtin {
                    constructor: B::CallableDefinition,
                    arguments,
                },
            ) => {
                if input.facts.known_std_types.code.callable_definition
                    != Some(TypeId(constructor.0))
                    || args.len() != arguments.len()
                {
                    return Ok(false);
                }
                children.extend(args.iter().copied().zip(arguments.iter().copied()));
            }
            (
                Type::Applied {
                    constructor: actual,
                    args,
                },
                TypeInfo::Applied {
                    constructor,
                    arguments,
                },
            ) => {
                if args.len() != arguments.len() {
                    return Ok(false);
                }
                children.push((TypeId(actual.0), *constructor));
                children.extend(args.iter().copied().zip(arguments.iter().copied()));
            }
            (Type::Function(actual), TypeInfo::Function(expected)) => {
                if !expected.generics.is_empty() || actual.input.len() != expected.parameters.len()
                {
                    return Ok(false);
                }
                if !rows_match(
                    input,
                    graph,
                    actual.effects.as_ref(),
                    expected.effects,
                    environment,
                )? {
                    return Ok(false);
                }
                children.extend(
                    actual
                        .input
                        .iter()
                        .copied()
                        .zip(expected.parameters.iter().map(|parameter| parameter.ty)),
                );
                children.push((actual.output, expected.output));
            }
            (
                Type::Handler(actual),
                TypeInfo::Handler {
                    handled,
                    produced,
                    result,
                },
            ) => {
                if !rows_match(
                    input,
                    graph,
                    Some(&actual.handled),
                    Some(*handled),
                    environment,
                )? {
                    return Ok(false);
                }
                match (&actual.produced, produced) {
                    (crate::HandlerProducedEffects::Infer, None) => {}
                    (crate::HandlerProducedEffects::Explicit(actual), Some(expected))
                        if rows_match(
                            input,
                            graph,
                            Some(actual),
                            Some(*expected),
                            environment,
                        )? => {}
                    _ => return Ok(false),
                }
                match (actual.result, result) {
                    (None, None) => {}
                    (Some(actual), Some(expected)) => children.push((actual, *expected)),
                    _ => return Ok(false),
                }
            }
            (
                _,
                TypeInfo::Builtin {
                    constructor,
                    arguments,
                },
            ) => {
                let Some((kind, actual)) = builtin(ty) else {
                    return Ok(false);
                };
                if kind != *constructor || actual.len() != arguments.len() {
                    return Ok(false);
                }
                children.extend(actual.into_iter().zip(arguments.iter().copied()));
            }
            _ => return Ok(false),
        }
        pending.extend(
            children
                .into_iter()
                .map(|(actual, expected)| (actual, expected, scope)),
        );
    }
    Ok(true)
}

fn rows_match(
    input: &TypeMatchInput<'_>,
    graph: &DescriptorGraphInput,
    actual: Option<&crate::EffectRowRef>,
    expected: Option<etas_code::descriptor::EffectRowInfoRef>,
    environment: &BTreeMap<GenericParameterRef, TypeInfoRef>,
) -> Result<bool, &'static str> {
    match (actual, expected) {
        (None, None) => Ok(true),
        (Some(actual), Some(expected)) => {
            let expected = graph
                .rows
                .get(expected.0 as usize)
                .ok_or("selected spec has a missing effect row descriptor")?;
            let tails_match = match (&actual.tail, expected.tail) {
                (None, None) => true,
                (Some(actual), Some(expected)) => {
                    input
                        .native_effect_binders
                        .and_then(|binders| binders.get(&expected))
                        == Some(actual)
                }
                _ => false,
            };
            if !tails_match {
                return Ok(false);
            }
            let mut covered = vec![false; expected.effects.len()];
            for actual in &actual.effects {
                let mut matched = false;
                for (index, expected) in expected.effects.iter().enumerate() {
                    if effect_matches(input, graph, actual, expected, environment)? {
                        covered[index] = true;
                        matched = true;
                    }
                }
                if !matched {
                    return Ok(false);
                }
            }
            Ok(covered.into_iter().all(|covered| covered))
        }
        _ => Ok(false),
    }
}

fn effect_matches(
    input: &TypeMatchInput<'_>,
    graph: &DescriptorGraphInput,
    actual: &crate::EffectRef,
    expected: &etas_code::descriptor::EffectInfo,
    environment: &BTreeMap<GenericParameterRef, TypeInfoRef>,
) -> Result<bool, &'static str> {
    use crate::EffectArgRef as A;
    use etas_code::descriptor::StaticEffectArgInfo as E;
    if input.facts.reflection_effect_targets.get(&actual.name) != Some(&expected.target)
        || actual.args.len() != expected.arguments.len()
    {
        return Ok(false);
    }
    for (actual, expected) in actual.args.iter().zip(&expected.arguments) {
        let matched = match (actual, expected) {
            (A::Type(actual), E::Type(expected)) => {
                matches_scoped(input, graph, *actual, *expected, environment)?
            }
            (A::Path(actual), E::Resource(expected)) => {
                input.facts.reflection_declaration_identities.get(actual) == Some(expected)
            }
            (A::String(actual), E::String(expected)) | (A::Int(actual), E::Integer(expected)) => {
                actual == expected
            }
            (A::Wildcard, E::Wildcard) => true,
            _ => false,
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn primitive(value: N) -> P {
    match value {
        N::Unit => P::Unit,
        N::Never => P::Never,
        N::Bool => P::Bool,
        N::I8 => P::I8,
        N::I16 => P::I16,
        N::I32 => P::I32,
        N::I64 => P::I64,
        N::I128 => P::I128,
        N::ISize => P::Isize,
        N::U8 => P::U8,
        N::U16 => P::U16,
        N::U32 => P::U32,
        N::U64 => P::U64,
        N::U128 => P::U128,
        N::USize => P::Usize,
        N::F32 => P::F32,
        N::F64 => P::F64,
        N::Char => P::Char,
        N::String => P::String,
        N::Bytes => P::Bytes,
    }
}

fn builtin(ty: &Type) -> Option<(B, Vec<TypeId>)> {
    Some(match ty {
        Type::Array(ty) => (B::Array, vec![*ty]),
        Type::List(ty) => (B::List, vec![*ty]),
        Type::Map { key, value } => (B::Map, vec![*key, *value]),
        Type::Set(ty) => (B::Set, vec![*ty]),
        Type::Range { index } => (B::Range, vec![*index]),
        Type::Slice(ty) => (B::Slice, vec![*ty]),
        Type::Option(ty) => (B::Option, vec![*ty]),
        Type::Result { ok, err } => (B::Result, vec![*ok, *err]),
        Type::Schema(ty) => (B::Schema, vec![*ty]),
        Type::Prompt => (B::Prompt, vec![]),
        Type::PromptPart => (B::PromptPart, vec![]),
        Type::Message(ty) => (B::Message, vec![*ty]),
        Type::MemorySelection(ty) => (B::MemorySelection, vec![*ty]),
        Type::Store { key, value } => (B::Store, vec![*key, *value]),
        Type::MemoryRegion(ty) => (B::MemoryRegion, vec![*ty]),
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
            (B::MemoryRegionHandle, vec![*schema])
        }
        Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
            (B::ExternalToolHandle, vec![*signature])
        }
        Type::Trust { wrapper, inner } => (
            match wrapper {
                TrustWrapper::Trusted => B::Trusted,
                TrustWrapper::Untrusted => B::Untrusted,
                TrustWrapper::Secret => B::Secret,
                TrustWrapper::Public => B::Public,
                TrustWrapper::Sanitized => B::Sanitized,
            },
            vec![*inner],
        ),
        _ => return None,
    })
}
