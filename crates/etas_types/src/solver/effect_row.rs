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
    pub deferred_fixed: Vec<(String, EffectRowRef)>,
    pub concrete: Vec<(String, crate::EffectRowEquation)>,
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
    infer_scoped_bindings_from_types(
        store,
        expected,
        actual,
        type_bindings,
        substitutions,
        EffectRowParameterScopes {
            inferred: params,
            lexical: &[],
        },
        bindings,
    )
}

pub(super) struct EffectRowParameterScopes<'a> {
    pub inferred: &'a [String],
    pub lexical: &'a [String],
}

pub(super) fn infer_scoped_bindings_from_types(
    store: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
    scopes: EffectRowParameterScopes<'_>,
    bindings: &mut HashMap<String, EffectRowRef>,
) -> EffectRowInference {
    let mut results = infer_scoped_bindings_from_pairs(
        store,
        &[(expected, actual)],
        type_bindings,
        substitutions,
        scopes,
        bindings,
    );
    results.remove(0)
}

pub(super) fn infer_bindings_from_type_pairs(
    store: &TypeStore,
    pairs: &[(TypeId, TypeId)],
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
    scopes: EffectRowParameterScopes<'_>,
    bindings: &mut HashMap<String, EffectRowRef>,
) -> Vec<EffectRowInference> {
    infer_scoped_bindings_from_pairs(store, pairs, type_bindings, substitutions, scopes, bindings)
}

fn infer_scoped_bindings_from_pairs(
    store: &TypeStore,
    pairs: &[(TypeId, TypeId)],
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
    scopes: EffectRowParameterScopes<'_>,
    bindings: &mut HashMap<String, EffectRowRef>,
) -> Vec<EffectRowInference> {
    let inputs = InferenceInputs {
        store,
        type_bindings,
        substitutions,
        params: scopes.inferred,
        lexical_params: scopes.lexical,
    };
    let mut state = InferenceState {
        bindings,
        results: vec![
            EffectRowInference {
                matches: true,
                ..EffectRowInference::default()
            };
            pairs.len()
        ],
        current: 0,
        equations: Vec::new(),
        visited: HashSet::new(),
    };
    for (index, &(expected, actual)) in pairs.iter().enumerate() {
        state.current = index;
        state.visited.clear();
        infer_type(&inputs, &mut state, expected, actual);
    }
    solve_row_equations(&inputs, &mut state);
    state.results
}

struct InferenceInputs<'a> {
    store: &'a TypeStore,
    type_bindings: &'a HashMap<String, TypeId>,
    substitutions: &'a Substitution,
    params: &'a [String],
    lexical_params: &'a [String],
}

struct InferenceState<'a> {
    bindings: &'a mut HashMap<String, EffectRowRef>,
    results: Vec<EffectRowInference>,
    current: usize,
    equations: Vec<RowEquation>,
    visited: HashSet<(TypeId, TypeId)>,
}

struct RowEquation {
    parameter: String,
    fixed: Vec<EffectRef>,
    actual: EffectRowRef,
    origin: usize,
}

fn infer_type(
    inputs: &InferenceInputs<'_>,
    state: &mut InferenceState<'_>,
    expected: TypeId,
    actual: TypeId,
) {
    if !state.results[state.current].matches {
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
        (Some(Type::Task(expected)), Some(Type::Task(actual))) => {
            recurse!(expected.output, actual.output);
            infer_row_binding(
                inputs,
                state,
                Some(&expected.effects),
                Some(&actual.effects),
            );
        }
        (Some(Type::Continuation(expected)), Some(Type::Continuation(actual))) => {
            recurse!(expected.action_output, actual.action_output);
            if let (Some(expected), Some(actual)) = (expected.handled_answer, actual.handled_answer)
            {
                recurse!(expected, actual);
            }
            infer_row_binding(
                inputs,
                state,
                expected.residual_effects.as_ref(),
                actual.residual_effects.as_ref(),
            );
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
    if actual.is_none()
        && expected.effects.is_empty()
        && let Some(tail) = expected
            .tail
            .as_ref()
            .filter(|tail| inputs.params.contains(tail))
    {
        // An unannotated callable has an unsolved latent row, not a proven
        // empty row. Its expression origin is solved by effect analysis.
        state.results[state.current].deferred.insert(tail.clone());
        state.results[state.current]
            .deferred_fixed
            .push((tail.clone(), empty_row()));
        return;
    }
    let actual = actual.cloned().unwrap_or_else(empty_row);
    let Some(tail) = expected
        .tail
        .as_ref()
        .filter(|tail| inputs.params.contains(tail))
    else {
        state.results[state.current].matches &= rows_equivalent(
            inputs.store,
            expected,
            &actual,
            inputs.type_bindings,
            inputs.substitutions,
        );
        return;
    };
    state.equations.push(RowEquation {
        parameter: tail.clone(),
        fixed: expected.effects.clone(),
        actual,
        origin: state.current,
    });
}

fn solve_row_equations(inputs: &InferenceInputs<'_>, state: &mut InferenceState<'_>) {
    let equivalent = |lhs: &EffectRef, rhs: &EffectRef| {
        effect_refs_equivalent(
            inputs.store,
            lhs,
            rhs,
            inputs.type_bindings,
            inputs.substitutions,
        )
    };
    let equations = std::mem::take(&mut state.equations);
    let mut grouped: std::collections::BTreeMap<&str, Vec<&RowEquation>> =
        std::collections::BTreeMap::new();
    for equation in &equations {
        if equation.actual.tail.as_deref() != Some(equation.parameter.as_str())
            || inputs.lexical_params.contains(&equation.parameter)
        {
            state.results[equation.origin].concrete.push((
                equation.parameter.clone(),
                crate::EffectRowEquation {
                    fixed: EffectRowRef {
                        effects: equation.fixed.clone(),
                        tail: None,
                    },
                    actual: equation.actual.clone(),
                },
            ));
        }
        grouped
            .entry(&equation.parameter)
            .or_default()
            .push(equation);
    }
    for (parameter, group) in grouped {
        let explicit = state.bindings.get(parameter).cloned();
        let mut inferred: Option<EffectRowRef> = None;
        for equation in &group {
            if !equation.fixed.iter().all(|fixed| {
                equation
                    .actual
                    .effects
                    .iter()
                    .any(|actual| equivalent(fixed, actual))
            }) {
                state.results[equation.origin].matches = false;
                continue;
            }
            let residual = equation
                .actual
                .effects
                .iter()
                .filter(|actual| !equation.fixed.iter().any(|fixed| equivalent(fixed, actual)))
                .cloned()
                .collect::<Vec<_>>();
            // Same-spelled caller and callee binders are distinct only when
            // the former is a checked lexical parameter.
            if equation.actual.tail.as_deref() == Some(parameter)
                && !inputs.lexical_params.iter().any(|name| name == parameter)
            {
                if residual.is_empty() {
                    state.results[equation.origin]
                        .deferred
                        .insert(parameter.to_owned());
                    state.results[equation.origin].deferred_fixed.push((
                        parameter.to_owned(),
                        EffectRowRef {
                            effects: equation.fixed.clone(),
                            tail: None,
                        },
                    ));
                } else {
                    state.results[equation.origin].matches = false;
                }
                continue;
            }
            if explicit.is_none() {
                let row = inferred.get_or_insert_with(|| EffectRowRef {
                    effects: Vec::new(),
                    tail: equation.actual.tail.clone(),
                });
                if row.tail != equation.actual.tail {
                    state.results[equation.origin].matches = false;
                }
                for effect in residual {
                    if !row
                        .effects
                        .iter()
                        .any(|existing| equivalent(existing, &effect))
                    {
                        row.effects.push(effect);
                    }
                }
            }
        }
        let Some(bound) = explicit.or(inferred) else {
            continue;
        };
        for equation in &group {
            if equation.actual.tail.as_deref() == Some(parameter)
                && !inputs.lexical_params.iter().any(|name| name == parameter)
            {
                continue;
            }
            let mut combined = equation.fixed.clone();
            for effect in &bound.effects {
                if !combined.iter().any(|fixed| equivalent(fixed, effect)) {
                    combined.push(effect.clone());
                }
            }
            let actual = &equation.actual;
            let matches = bound.tail == actual.tail
                && combined
                    .iter()
                    .all(|effect| actual.effects.iter().any(|a| equivalent(effect, a)))
                && actual
                    .effects
                    .iter()
                    .all(|effect| combined.iter().any(|a| equivalent(effect, a)));
            state.results[equation.origin].matches &= matches;
        }
        if group
            .iter()
            .all(|equation| state.results[equation.origin].matches)
        {
            state.bindings.insert(parameter.to_owned(), bound);
        }
    }
}

fn rows_equivalent(
    store: &TypeStore,
    lhs: &EffectRowRef,
    rhs: &EffectRowRef,
    type_bindings: &HashMap<String, TypeId>,
    substitutions: &Substitution,
) -> bool {
    lhs.tail == rhs.tail
        && lhs.effects.iter().all(|lhs| {
            rhs.effects
                .iter()
                .any(|rhs| effect_refs_equivalent(store, lhs, rhs, type_bindings, substitutions))
        })
        && rhs.effects.iter().all(|rhs| {
            lhs.effects
                .iter()
                .any(|lhs| effect_refs_equivalent(store, lhs, rhs, type_bindings, substitutions))
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
        (Some(Type::Task(lhs)), Some(Type::Task(rhs))) => {
            recurse!(lhs.output, rhs.output)
                && rows_equivalent(
                    store,
                    &lhs.effects,
                    &rhs.effects,
                    type_bindings,
                    substitutions,
                )
        }
        (Some(Type::SchedulerHandle), Some(Type::SchedulerHandle)) => true,
        (Some(Type::Continuation(lhs)), Some(Type::Continuation(rhs))) => {
            recurse!(lhs.action_output, rhs.action_output)
                && match (lhs.handled_answer, rhs.handled_answer) {
                    (Some(lhs), Some(rhs)) => recurse!(lhs, rhs),
                    (None, None) => true,
                    _ => false,
                }
                && match (&lhs.residual_effects, &rhs.residual_effects) {
                    (Some(lhs), Some(rhs)) => {
                        rows_equivalent(store, lhs, rhs, type_bindings, substitutions)
                    }
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
        (Some(left @ Type::Nominal(_)), Some(right @ Type::Nominal(_)))
        | (Some(left @ Type::Enum(_)), Some(right @ Type::Enum(_))) => {
            match (store.nominal_identity(lhs), store.nominal_identity(rhs)) {
                (Some(left), Some(right)) => left == right,
                (None, None) => left == right,
                _ => false,
            }
        }
        (Some(Type::Named(left)), Some(Type::Named(right))) => {
            match (store.parameter_identity(lhs), store.parameter_identity(rhs)) {
                (Some(left), Some(right)) => left == right,
                (None, None) => left == right,
                _ => false,
            }
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
            Some(Type::Named(name)) if store.parameter_identity(current).is_none() => {
                type_bindings.get(&name.name).copied()
            }
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

#[cfg(test)]
mod tests;
