use std::collections::{BTreeSet, HashMap, HashSet};

use crate::{
    EffectArgRef, EffectRef, EffectRowRef, HandlerProducedEffects, ResourceHandleType, Type,
    TypeId, TypeStore,
};

use super::Substitution;

pub fn rows_equal(lhs: &EffectRowRef, rhs: &EffectRowRef) -> bool {
    lhs == rhs
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectRowInference {
    pub matches: bool,
    pub deferred: BTreeSet<String>,
}

pub fn infer_bindings_from_types(
    store: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
    params: &[String],
    bindings: &mut HashMap<String, EffectRowRef>,
) -> EffectRowInference {
    let inputs = InferenceInputs {
        store,
        type_bindings,
        substitutions,
        params,
    };
    let mut state = InferenceState {
        bindings,
        result: EffectRowInference {
            matches: true,
            deferred: BTreeSet::new(),
        },
        visited: HashSet::new(),
    };
    infer_type(&inputs, &mut state, expected, actual);
    state.result
}

struct InferenceInputs<'a> {
    store: &'a TypeStore,
    type_bindings: &'a HashMap<String, TypeId>,
    substitutions: &'a Substitution,
    params: &'a [String],
}

struct InferenceState<'a> {
    bindings: &'a mut HashMap<String, EffectRowRef>,
    result: EffectRowInference,
    visited: HashSet<(TypeId, TypeId)>,
}

fn infer_type(
    inputs: &InferenceInputs<'_>,
    state: &mut InferenceState<'_>,
    expected: TypeId,
    actual: TypeId,
) {
    if !state.result.matches {
        return;
    }
    let expected = resolve_type(
        inputs.store,
        expected,
        inputs.type_bindings,
        inputs.substitutions,
    );
    let actual = resolve_type(
        inputs.store,
        actual,
        inputs.type_bindings,
        inputs.substitutions,
    );
    if !state.visited.insert((expected, actual)) {
        return;
    }
    macro_rules! recurse {
        ($expected:expr, $actual:expr) => {
            infer_type(inputs, state, $expected, $actual)
        };
    }
    match (inputs.store.get(expected), inputs.store.get(actual)) {
        (Some(Type::Function(expected)), Some(Type::Function(actual))) => {
            if expected.input.len() != actual.input.len() {
                return;
            }
            for (expected, actual) in expected.input.iter().zip(&actual.input) {
                recurse!(*expected, *actual);
            }
            recurse!(expected.output, actual.output);
            infer_row_binding(
                inputs,
                state,
                expected.effects.as_ref(),
                actual.effects.as_ref(),
            );
        }
        (Some(Type::Handler(expected)), Some(Type::Handler(actual))) => {
            infer_row_binding(
                inputs,
                state,
                Some(&expected.handled),
                Some(&actual.handled),
            );
            if let (
                HandlerProducedEffects::Explicit(expected),
                HandlerProducedEffects::Explicit(actual),
            ) = (&expected.produced, &actual.produced)
            {
                infer_row_binding(inputs, state, Some(expected), Some(actual));
            }
            if let (Some(expected), Some(actual)) = (expected.result, actual.result) {
                recurse!(expected, actual);
            }
        }
        (Some(Type::Record(expected)), Some(Type::Record(actual))) => {
            for expected in &expected.fields {
                if let Some(actual) = actual
                    .fields
                    .iter()
                    .find(|actual| actual.name == expected.name)
                {
                    recurse!(expected.ty, actual.ty);
                }
            }
        }
        (Some(Type::Tuple(expected)), Some(Type::Tuple(actual)))
            if expected.len() == actual.len() =>
        {
            for (expected, actual) in expected.iter().zip(actual) {
                recurse!(*expected, *actual);
            }
        }
        (
            Some(Type::Applied {
                constructor: expected_constructor,
                args: expected,
            }),
            Some(Type::Applied {
                constructor: actual_constructor,
                args: actual,
            }),
        ) if expected_constructor == actual_constructor && expected.len() == actual.len() => {
            for (expected, actual) in expected.iter().zip(actual) {
                recurse!(*expected, *actual);
            }
        }
        (Some(Type::Array(expected)), Some(Type::Array(actual)))
        | (Some(Type::List(expected)), Some(Type::List(actual)))
        | (Some(Type::Set(expected)), Some(Type::Set(actual)))
        | (Some(Type::Slice(expected)), Some(Type::Slice(actual)))
        | (Some(Type::Option(expected)), Some(Type::Option(actual)))
        | (Some(Type::Schema(expected)), Some(Type::Schema(actual)))
        | (Some(Type::Message(expected)), Some(Type::Message(actual)))
        | (Some(Type::MemorySelection(expected)), Some(Type::MemorySelection(actual)))
        | (Some(Type::MemoryRegion(expected)), Some(Type::MemoryRegion(actual)))
        | (Some(Type::Range { index: expected }), Some(Type::Range { index: actual })) => {
            recurse!(*expected, *actual);
        }
        (
            Some(Type::Refined {
                base: expected,
                predicate: expected_predicate,
            }),
            Some(Type::Refined {
                base: actual,
                predicate: actual_predicate,
            }),
        ) if expected_predicate == actual_predicate => recurse!(*expected, *actual),
        (
            Some(Type::Trust {
                wrapper: expected_wrapper,
                inner: expected,
            }),
            Some(Type::Trust {
                wrapper: actual_wrapper,
                inner: actual,
            }),
        ) if expected_wrapper == actual_wrapper => recurse!(*expected, *actual),
        (
            Some(Type::Map {
                key: expected_key,
                value: expected_value,
            }),
            Some(Type::Map {
                key: actual_key,
                value: actual_value,
            }),
        )
        | (
            Some(Type::Store {
                key: expected_key,
                value: expected_value,
            }),
            Some(Type::Store {
                key: actual_key,
                value: actual_value,
            }),
        )
        | (
            Some(Type::Result {
                ok: expected_key,
                err: expected_value,
            }),
            Some(Type::Result {
                ok: actual_key,
                err: actual_value,
            }),
        ) => {
            recurse!(*expected_key, *actual_key);
            recurse!(*expected_value, *actual_value);
        }
        (
            Some(Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema: expected })),
            Some(Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema: actual })),
        )
        | (
            Some(Type::ResourceHandle(ResourceHandleType::ExternalTool {
                signature: expected,
            })),
            Some(Type::ResourceHandle(ResourceHandleType::ExternalTool { signature: actual })),
        ) => recurse!(*expected, *actual),
        (
            Some(Type::ResourceHandle(ResourceHandleType::Other {
                name: expected_name,
                args: expected,
            })),
            Some(Type::ResourceHandle(ResourceHandleType::Other {
                name: actual_name,
                args: actual,
            })),
        ) if expected_name == actual_name && expected.len() == actual.len() => {
            for (expected, actual) in expected.iter().zip(actual) {
                recurse!(*expected, *actual);
            }
        }
        _ => {}
    }
}

fn infer_row_binding(
    inputs: &InferenceInputs<'_>,
    state: &mut InferenceState<'_>,
    expected: Option<&EffectRowRef>,
    actual: Option<&EffectRowRef>,
) {
    let Some(expected) = expected else {
        return;
    };
    let actual = actual.cloned().unwrap_or_else(empty_row);
    let Some(tail) = expected
        .tail
        .as_ref()
        .filter(|tail| inputs.params.contains(tail))
    else {
        state.result.matches &= rows_equivalent(
            inputs.store,
            expected,
            &actual,
            inputs.type_bindings,
            inputs.substitutions,
        );
        return;
    };
    let residual_effects =
        expected
            .effects
            .iter()
            .try_fold(actual.effects.clone(), |mut remaining, expected| {
                let index = remaining.iter().position(|actual| {
                    effect_refs_equivalent(
                        inputs.store,
                        expected,
                        actual,
                        inputs.type_bindings,
                        inputs.substitutions,
                    )
                })?;
                remaining.remove(index);
                Some(remaining)
            });
    let Some(residual_effects) = residual_effects else {
        state.result.matches = false;
        return;
    };
    if let Some(bound) = state.bindings.get(tail) {
        let residual = EffectRowRef {
            effects: residual_effects,
            tail: actual.tail.clone(),
        };
        state.result.matches &= rows_equivalent(
            inputs.store,
            bound,
            &residual,
            inputs.type_bindings,
            inputs.substitutions,
        );
        return;
    }
    if actual.tail.as_deref() == Some(tail) {
        if residual_effects.is_empty() {
            state.result.deferred.insert(tail.clone());
        } else {
            state.result.matches = false;
        }
        return;
    }
    state.bindings.insert(
        tail.clone(),
        EffectRowRef {
            effects: residual_effects,
            tail: actual.tail.clone(),
        },
    );
}

fn rows_equivalent(
    store: &TypeStore,
    lhs: &EffectRowRef,
    rhs: &EffectRowRef,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
) -> bool {
    lhs.tail == rhs.tail
        && lhs.effects.len() == rhs.effects.len()
        && lhs.effects.iter().all(|lhs| {
            rhs.effects
                .iter()
                .any(|rhs| effect_refs_equivalent(store, lhs, rhs, type_bindings, substitutions))
        })
}

fn effect_refs_equivalent(
    store: &TypeStore,
    lhs: &EffectRef,
    rhs: &EffectRef,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
) -> bool {
    lhs.name == rhs.name
        && lhs.args.len() == rhs.args.len()
        && lhs
            .args
            .iter()
            .zip(&rhs.args)
            .all(|(lhs, rhs)| match (lhs, rhs) {
                (EffectArgRef::Type(lhs), EffectArgRef::Type(rhs)) => types_equivalent(
                    store,
                    *lhs,
                    *rhs,
                    type_bindings,
                    substitutions,
                    &mut HashSet::new(),
                ),
                _ => lhs == rhs,
            })
}

fn types_equivalent(
    store: &TypeStore,
    lhs: TypeId,
    rhs: TypeId,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
    visited: &mut HashSet<(TypeId, TypeId)>,
) -> bool {
    let lhs = resolve_type(store, lhs, type_bindings, substitutions);
    let rhs = resolve_type(store, rhs, type_bindings, substitutions);
    if lhs == rhs || !visited.insert((lhs, rhs)) {
        return true;
    }
    macro_rules! recurse {
        ($lhs:expr, $rhs:expr) => {
            types_equivalent(store, $lhs, $rhs, type_bindings, substitutions, visited)
        };
    }
    match (store.get(lhs), store.get(rhs)) {
        (Some(Type::Array(lhs)), Some(Type::Array(rhs)))
        | (Some(Type::List(lhs)), Some(Type::List(rhs)))
        | (Some(Type::Set(lhs)), Some(Type::Set(rhs)))
        | (Some(Type::Slice(lhs)), Some(Type::Slice(rhs)))
        | (Some(Type::Option(lhs)), Some(Type::Option(rhs)))
        | (Some(Type::Schema(lhs)), Some(Type::Schema(rhs)))
        | (Some(Type::Message(lhs)), Some(Type::Message(rhs)))
        | (Some(Type::MemorySelection(lhs)), Some(Type::MemorySelection(rhs)))
        | (Some(Type::MemoryRegion(lhs)), Some(Type::MemoryRegion(rhs))) => {
            recurse!(*lhs, *rhs)
        }
        (Some(Type::Range { index: lhs }), Some(Type::Range { index: rhs })) => {
            recurse!(*lhs, *rhs)
        }
        (Some(Type::Tuple(lhs)), Some(Type::Tuple(rhs))) => {
            lhs.len() == rhs.len() && lhs.iter().zip(rhs).all(|(lhs, rhs)| recurse!(*lhs, *rhs))
        }
        (
            Some(Type::Applied {
                constructor: lhs_constructor,
                args: lhs,
            }),
            Some(Type::Applied {
                constructor: rhs_constructor,
                args: rhs,
            }),
        ) => {
            lhs_constructor == rhs_constructor
                && lhs.len() == rhs.len()
                && lhs.iter().zip(rhs).all(|(lhs, rhs)| recurse!(*lhs, *rhs))
        }
        (Some(Type::Record(lhs)), Some(Type::Record(rhs))) => {
            lhs.fields.len() == rhs.fields.len()
                && lhs
                    .fields
                    .iter()
                    .zip(&rhs.fields)
                    .all(|(lhs, rhs)| lhs.name == rhs.name && recurse!(lhs.ty, rhs.ty))
        }
        (Some(Type::Function(lhs)), Some(Type::Function(rhs))) => {
            lhs.input.len() == rhs.input.len()
                && lhs
                    .input
                    .iter()
                    .zip(&rhs.input)
                    .all(|(lhs, rhs)| recurse!(*lhs, *rhs))
                && recurse!(lhs.output, rhs.output)
                && match (&lhs.effects, &rhs.effects) {
                    (Some(lhs), Some(rhs)) => {
                        rows_equivalent(store, lhs, rhs, type_bindings, substitutions)
                    }
                    (None, None) => true,
                    _ => false,
                }
        }
        (Some(Type::Handler(lhs)), Some(Type::Handler(rhs))) => {
            rows_equivalent(
                store,
                &lhs.handled,
                &rhs.handled,
                type_bindings,
                substitutions,
            ) && match (&lhs.produced, &rhs.produced) {
                (HandlerProducedEffects::Infer, HandlerProducedEffects::Infer) => true,
                (HandlerProducedEffects::Explicit(lhs), HandlerProducedEffects::Explicit(rhs)) => {
                    rows_equivalent(store, lhs, rhs, type_bindings, substitutions)
                }
                _ => false,
            } && match (lhs.result, rhs.result) {
                (Some(lhs), Some(rhs)) => recurse!(lhs, rhs),
                (None, None) => true,
                _ => false,
            }
        }
        (
            Some(Type::Map {
                key: lhs_key,
                value: lhs_value,
            }),
            Some(Type::Map {
                key: rhs_key,
                value: rhs_value,
            }),
        )
        | (
            Some(Type::Store {
                key: lhs_key,
                value: lhs_value,
            }),
            Some(Type::Store {
                key: rhs_key,
                value: rhs_value,
            }),
        )
        | (
            Some(Type::Result {
                ok: lhs_key,
                err: lhs_value,
            }),
            Some(Type::Result {
                ok: rhs_key,
                err: rhs_value,
            }),
        ) => recurse!(*lhs_key, *rhs_key) && recurse!(*lhs_value, *rhs_value),
        (
            Some(Type::Refined {
                base: lhs,
                predicate: lhs_predicate,
            }),
            Some(Type::Refined {
                base: rhs,
                predicate: rhs_predicate,
            }),
        ) => lhs_predicate == rhs_predicate && recurse!(*lhs, *rhs),
        (
            Some(Type::Trust {
                wrapper: lhs_wrapper,
                inner: lhs,
            }),
            Some(Type::Trust {
                wrapper: rhs_wrapper,
                inner: rhs,
            }),
        ) => lhs_wrapper == rhs_wrapper && recurse!(*lhs, *rhs),
        (
            Some(Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema: lhs })),
            Some(Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema: rhs })),
        )
        | (
            Some(Type::ResourceHandle(ResourceHandleType::ExternalTool { signature: lhs })),
            Some(Type::ResourceHandle(ResourceHandleType::ExternalTool { signature: rhs })),
        ) => recurse!(*lhs, *rhs),
        (
            Some(Type::ResourceHandle(ResourceHandleType::Other {
                name: lhs_name,
                args: lhs,
            })),
            Some(Type::ResourceHandle(ResourceHandleType::Other {
                name: rhs_name,
                args: rhs,
            })),
        ) => {
            lhs_name == rhs_name
                && lhs.len() == rhs.len()
                && lhs.iter().zip(rhs).all(|(lhs, rhs)| recurse!(*lhs, *rhs))
        }
        (Some(lhs), Some(rhs)) => lhs == rhs,
        _ => false,
    }
}

fn resolve_type(
    store: &TypeStore,
    ty: TypeId,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
) -> TypeId {
    let mut current = ty;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        let next = match store.get(current) {
            Some(Type::Var(var)) => substitutions.get(*var),
            Some(Type::Named(name)) => type_bindings.get(&name.name).copied(),
            _ => None,
        };
        let Some(next) = next else {
            break;
        };
        current = next;
    }
    current
}

pub fn specialize_row(
    row: &EffectRowRef,
    bindings: &HashMap<String, EffectRowRef>,
) -> EffectRowRef {
    let Some(tail) = row.tail.as_ref() else {
        return row.clone();
    };
    let Some(bound) = bindings.get(tail) else {
        return row.clone();
    };
    let mut effects = row.effects.clone();
    for effect in &bound.effects {
        if !effects.contains(effect) {
            effects.push(effect.clone());
        }
    }
    EffectRowRef {
        effects,
        tail: bound.tail.clone(),
    }
}

fn empty_row() -> EffectRowRef {
    EffectRowRef {
        effects: Vec::new(),
        tail: None,
    }
}
