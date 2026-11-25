use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use etas_hir::HirExprId;

use crate::{EffectArgRef, EffectRowRef, TypeOutput};

pub struct ResolvedEffectRowInstantiation {
    pub call: HirExprId,
    pub bindings: Vec<(String, EffectRowRef)>,
}

pub struct ResolvedTryCapture {
    pub expression: HirExprId,
    pub error: crate::TypeId,
}

/// Complete the Result error variables that waited for a callback's Effect row.
/// Apply the checked unification to every dependent fact, including method
/// owner arguments selected before Effects could close that row.
pub fn materialize_try_captures(
    output: &mut TypeOutput,
    captures: &[ResolvedTryCapture],
) -> Result<bool, EffectRowMaterializationError> {
    let mut substitutions = crate::Substitution::default();
    for capture in captures {
        let error = |reason| EffectRowMaterializationError {
            call: capture.expression,
            reason,
        };
        let fact = output
            .facts
            .try_facts
            .get(&capture.expression)
            .ok_or_else(|| error("captured Error has no checked try type fact"))?;
        let target = fact
            .target_error
            .or_else(|| {
                crate::value_representation::result_parameters(
                    &output.store,
                    &output.facts.known_std_types,
                    fact.result_type,
                )
                .map(|(_, error)| error)
            })
            .ok_or_else(|| error("captured Error has no Result error type"))?;
        let mut unifier =
            crate::TypeUnifier::with_known_substitution(&output.store, &substitutions);
        unifier
            .unify(target, capture.error)
            .map_err(|_| error("captured Error conflicts with the checked Result error type"))?;
        substitutions.extend(&unifier.into_substitution());
    }
    if substitutions.is_empty() {
        return Ok(false);
    }
    let mut facts = output.facts.as_ref().clone();
    let mut interner = crate::TypeInterner::from_store(output.store.as_ref().clone());
    super::remap::substitute_facts(&mut facts, &mut interner, &substitutions).map_err(|_| {
        EffectRowMaterializationError {
            call: captures[0].expression,
            reason: "captured Error could not complete its dependent checked types",
        }
    })?;
    output.facts = Arc::new(facts);
    output.store = Arc::new(interner.into_store());
    Ok(true)
}

#[derive(Debug, PartialEq, Eq)]
pub struct EffectRowMaterializationError {
    pub call: HirExprId,
    pub reason: &'static str,
}

impl std::fmt::Display for EffectRowMaterializationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason)
    }
}

impl std::error::Error for EffectRowMaterializationError {}

pub fn materialize_effect_row_instantiations(
    output: &mut TypeOutput,
    patches: &[ResolvedEffectRowInstantiation],
) -> Result<(), EffectRowMaterializationError> {
    let mut calls = HashSet::new();
    for patch in patches {
        let error = |reason| EffectRowMaterializationError {
            call: patch.call,
            reason,
        };
        if !calls.insert(patch.call) {
            return Err(error("duplicate solved effect-row call"));
        }
        let fact = output
            .facts
            .generic_instantiations
            .get(&patch.call)
            .ok_or_else(|| error("solved effect row has no checked generic instantiation"))?;
        let expected = fact
            .deferred_effect_row_obligations
            .iter()
            .map(|obligation| obligation.param.as_str())
            .collect::<HashSet<_>>();
        let names = patch
            .bindings
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<HashSet<_>>();
        if expected.is_empty() || expected != names || names.len() != patch.bindings.len() {
            return Err(error(
                "solved effect rows do not match deferred generic binders",
            ));
        }
        for (name, row) in &patch.bindings {
            if name.is_empty()
                || row.tail.as_ref().is_some_and(String::is_empty)
                || row.effects.iter().any(|effect| {
                    effect.name.is_empty() || effect.args.iter().any(|arg| {
                    matches!(arg, EffectArgRef::Type(ty) if output.store.get(*ty).is_none())
                })
                })
            {
                return Err(error(
                    "solved generic row contains an invalid checked reference",
                ));
            }
            if fact
                .effect_row_bindings
                .iter()
                .any(|(previous_name, previous)| previous_name == name && previous != row)
            {
                return Err(error(
                    "solved effect row conflicts with the explicit generic binding",
                ));
            }
        }
    }
    if patches.is_empty() {
        return Ok(());
    }
    let mut interner = crate::TypeInterner::from_store(output.store.as_ref().clone());
    let mut source_types = HashMap::new();
    let mut method_calls = HashMap::new();
    let mut std_calls = HashMap::new();
    let names = HashMap::new();
    for patch in patches {
        let rows = patch.bindings.iter().cloned().collect::<HashMap<_, _>>();
        let fact = &output.facts.generic_instantiations[&patch.call];
        for source in fact
            .deferred_effect_row_obligations
            .iter()
            .map(|obligation| obligation.source)
        {
            let error = |reason| EffectRowMaterializationError {
                call: patch.call,
                reason,
            };
            let ty =
                *output.facts.expr_types.get(&source).ok_or_else(|| {
                    error("deferred callable source has no checked expression type")
                })?;
            let resolved =
                crate::TypeSubstitutionEngine::materializing(&mut interner, &names, None)
                    .with_effect_arguments(&rows)
                    .substitute(ty)
                    .map_err(|_| {
                        error("deferred callable type could not materialize its solved rows")
                    })?;
            if source_types
                .insert(source, resolved)
                .is_some_and(|previous| previous != resolved)
            {
                return Err(error(
                    "deferred callable source has conflicting solved types",
                ));
            }
        }
        if let Some(method) = output.facts.checked_method_calls.get(&patch.call) {
            let mut method = method.clone();
            for (name, row) in &patch.bindings {
                if !method
                    .instantiation
                    .effect_row_bindings
                    .iter()
                    .any(|(existing, _)| existing == name)
                {
                    method
                        .instantiation
                        .effect_row_bindings
                        .push((name.clone(), row.clone()));
                }
            }
            method.instantiation.deferred_effect_row_obligations.clear();
            let declaration = output
                .facts
                .methods
                .declaration(&method.target, &method.evidence)
                .ok_or(EffectRowMaterializationError {
                    call: patch.call,
                    reason: "checked method has no unique declaration for its solved rows",
                })?;
            method.signature =
                crate::method::instantiate::signature(&mut interner, declaration, &method)
                    .map_err(|_| EffectRowMaterializationError {
                        call: patch.call,
                        reason: "checked method ABI could not materialize its solved effect rows",
                    })?;
            source_types.insert(patch.call, method.signature.output);
            if let Some(fact) = output.facts.std_intrinsic_calls.get(&patch.call) {
                let mut fact = fact.clone();
                fact.signature = method.signature.clone();
                fact.callable_type = interner.intern(crate::Type::Function(crate::FlowType {
                    input: fact.signature.params.clone(),
                    output: fact.signature.output,
                    effects: fact.signature.effects.clone(),
                }));
                std_calls.insert(patch.call, fact);
            }
            method_calls.insert(patch.call, method);
        } else if let Some(call) = output.facts.std_intrinsic_calls.get(&patch.call) {
            let types = fact
                .type_bindings
                .iter()
                .cloned()
                .collect::<HashMap<_, _>>();
            for ty in call
                .signature
                .params
                .iter()
                .copied()
                .chain(std::iter::once(call.signature.output))
                .chain(std::iter::once(call.callable_type))
            {
                crate::TypeSubstitutionEngine::materializing(&mut interner, &types, None)
                    .with_effect_arguments(&rows)
                    .substitute(ty)
                    .map_err(|_| EffectRowMaterializationError {
                        call: patch.call,
                        reason: "checked intrinsic ABI could not materialize its solved effect rows",
                    })?;
            }
        }
    }
    // Validate the whole batch before publishing any completed checked facts.
    output.store = Arc::new(interner.into_store());
    let facts = Arc::make_mut(&mut output.facts);
    facts.expr_types.extend(source_types);
    facts.checked_method_calls.extend(method_calls);
    facts.std_intrinsic_calls.extend(std_calls);
    for patch in patches {
        let fact = facts.generic_instantiations.get_mut(&patch.call).ok_or(
            EffectRowMaterializationError {
                call: patch.call,
                reason: "validated generic instantiation disappeared during materialization",
            },
        )?;
        for (name, row) in &patch.bindings {
            if !fact
                .effect_row_bindings
                .iter()
                .any(|(existing, _)| existing == name)
            {
                fact.effect_row_bindings.push((name.clone(), row.clone()));
            }
        }
        fact.deferred_effect_row_obligations.clear();
    }
    Ok(())
}

/// Complete solved lambda rows and rederive selected method ABIs from their
/// checked declaration and arguments. Filling a row may make an inferred
/// function type equal to an earlier interned type used by the declaration.
pub fn materialize_lambda_effect_rows(
    output: &mut TypeOutput,
    rows: &HashMap<String, EffectRowRef>,
) -> Result<bool, EffectRowMaterializationError> {
    if rows.is_empty() {
        return Ok(false);
    }
    let mut store = output.store.as_ref().clone();
    if !store.resolve_lambda_effect_rows(rows) {
        return Ok(false);
    }
    let mut interner = crate::TypeInterner::from_store(store);
    // Calls outside the checked reachability may still wait for a row. They
    // keep their deferred obligation and are completed when that body is checked.
    let mut calls = output
        .facts
        .checked_method_calls
        .iter()
        .filter(|(_, fact)| {
            fact.instantiation
                .deferred_effect_row_obligations
                .is_empty()
        })
        .collect::<Vec<_>>();
    calls.sort_by_key(|(call, _)| **call);
    let mut methods = Vec::with_capacity(calls.len());
    let mut intrinsic_calls = Vec::new();
    for (call, fact) in calls {
        let error = |reason| EffectRowMaterializationError {
            call: *call,
            reason,
        };
        let declaration = output
            .facts
            .methods
            .declaration(&fact.target, &fact.evidence)
            .ok_or_else(|| error("completed lambda row has no checked method declaration"))?;
        let mut method = fact.clone();
        method.signature = crate::method::instantiate::signature(&mut interner, declaration, fact)
            .map_err(|_| error("completed lambda row could not rederive its checked method ABI"))?;
        if let Some(intrinsic) = output.facts.std_intrinsic_calls.get(call) {
            let mut intrinsic = intrinsic.clone();
            intrinsic.signature = method.signature.clone();
            intrinsic.callable_type = interner.intern(crate::Type::Function(crate::FlowType {
                input: method.signature.params.clone(),
                output: method.signature.output,
                effects: method.signature.effects.clone(),
            }));
            intrinsic_calls.push((*call, intrinsic));
        }
        methods.push((*call, method));
    }
    output.store = Arc::new(interner.into_store());
    let facts = Arc::make_mut(&mut output.facts);
    for (call, method) in methods {
        facts.expr_types.insert(call, method.signature.output);
        facts.checked_method_calls.insert(call, method);
    }
    facts.std_intrinsic_calls.extend(intrinsic_calls);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DeferredEffectRowObligation, EffectRef, GenericInstantiationFact, PrimitiveType,
        TypeInterner, facts::CheckedStdCallFact,
    };

    fn pending() -> TypeOutput {
        let mut interner = TypeInterner::new();
        let string = interner.primitive(PrimitiveType::String);
        let callback = interner.intern(crate::Type::Function(crate::FlowType {
            input: Vec::new(),
            output: string,
            effects: Some(EffectRowRef {
                effects: Vec::new(),
                tail: Some("E".into()),
            }),
        }));
        let mut output = TypeOutput {
            store: Arc::new(interner.into_store()),
            ..Default::default()
        };
        Arc::make_mut(&mut output.facts)
            .expr_types
            .insert(HirExprId(4), callback);
        Arc::make_mut(&mut output.facts)
            .generic_instantiations
            .insert(
                HirExprId(3),
                GenericInstantiationFact {
                    type_bindings: vec![("T".into(), string)],
                    effect_row_bindings: vec![],
                    deferred_effect_row_obligations: vec![DeferredEffectRowObligation {
                        param: "E".into(),
                        source: HirExprId(4),
                        fixed: crate::EffectRowRef {
                            effects: vec![],
                            tail: None,
                        },
                        concrete: vec![],
                    }],
                },
            );
        output
    }

    fn patch(name: &str) -> ResolvedEffectRowInstantiation {
        ResolvedEffectRowInstantiation {
            call: HirExprId(3),
            bindings: vec![(
                name.into(),
                EffectRowRef {
                    effects: vec![],
                    tail: None,
                },
            )],
        }
    }

    #[test]
    fn solved_rows_complete_facts_without_changing_type_bindings_or_previous_snapshots() {
        let mut output = pending();
        let old = output.facts.clone();
        let old_store = output.store.clone();
        materialize_effect_row_instantiations(&mut output, &[patch("E")]).unwrap();
        let fact = &output.facts.generic_instantiations[&HirExprId(3)];
        assert!(fact.deferred_effect_row_obligations.is_empty());
        assert_eq!(fact.effect_row_bindings, patch("E").bindings);
        let source = output.facts.expr_types[&HirExprId(4)];
        assert!(
            matches!(output.store.get(source), Some(crate::Type::Function(flow)) if flow.effects.as_ref().unwrap().tail.is_none())
        );
        assert!(
            matches!(old_store.get(old.expr_types[&HirExprId(4)]), Some(crate::Type::Function(flow)) if flow.effects.as_ref().unwrap().tail.as_deref() == Some("E"))
        );
        assert_eq!(
            fact.type_bindings,
            old.generic_instantiations[&HirExprId(3)].type_bindings
        );
        assert_eq!(
            old.generic_instantiations[&HirExprId(3)]
                .deferred_effect_row_obligations
                .len(),
            1
        );
    }

    #[test]
    fn solved_rows_materialize_direct_std_call_abi() {
        let mut output = pending();
        let mut interner = TypeInterner::from_store(output.store.as_ref().clone());
        let string = output.facts.generic_instantiations[&HirExprId(3)].type_bindings[0].1;
        let callback = output.facts.expr_types[&HirExprId(4)];
        let task = interner.intern(crate::Type::Task(crate::TaskType {
            output: string,
            effects: EffectRowRef {
                effects: vec![],
                tail: Some("E".into()),
            },
        }));
        let callable = interner.intern(crate::Type::Function(crate::FlowType {
            input: vec![callback],
            output: task,
            effects: Some(EffectRowRef {
                effects: vec![],
                tail: Some("E".into()),
            }),
        }));
        output.store = Arc::new(interner.into_store());
        Arc::make_mut(&mut output.facts).std_intrinsic_calls.insert(
            HirExprId(3),
            CheckedStdCallFact {
                qualified_path: vec!["std".into(), "task".into(), "start".into()],
                intrinsic: 0,
                overload: 0,
                callable_type: callable,
                signature: crate::CallableSignature {
                    parameter_interface: None,
                    params: vec![callback],
                    output: task,
                    effects: Some(EffectRowRef {
                        effects: vec![],
                        tail: Some("E".into()),
                    }),
                    generic_params: vec![],
                    requested_actions: None,
                },
            },
        );

        materialize_effect_row_instantiations(&mut output, &[patch("E")]).unwrap();

        let call = &output.facts.std_intrinsic_calls[&HirExprId(3)];
        let types = output.facts.generic_instantiations[&HirExprId(3)]
            .type_bindings
            .iter()
            .cloned()
            .collect::<HashMap<_, _>>();
        let rows = patch("E").bindings.into_iter().collect::<HashMap<_, _>>();
        let specialized = crate::TypeSubstitutionEngine::existing(&output.store, &types)
            .with_effect_arguments(&rows)
            .substitute(call.signature.output)
            .unwrap();
        assert!(matches!(
            output.store.get(specialized),
            Some(crate::Type::Task(task)) if task.effects.tail.is_none()
        ));
        assert!(!output.facts.expr_types.contains_key(&HirExprId(3)));
    }

    #[test]
    fn malformed_batches_do_not_publish_partial_facts() {
        let mut output = pending();
        let old = output.facts.clone();
        let mut absent = patch("E");
        absent.call = HirExprId(99);
        assert!(materialize_effect_row_instantiations(&mut output, &[patch("E"), absent]).is_err());
        assert!(Arc::ptr_eq(&old, &output.facts));
        for bindings in [
            vec![],
            patch("F").bindings,
            [patch("E").bindings, patch("E").bindings].concat(),
        ] {
            assert!(
                materialize_effect_row_instantiations(
                    &mut output,
                    &[ResolvedEffectRowInstantiation {
                        call: HirExprId(3),
                        bindings
                    }]
                )
                .is_err()
            );
            assert!(Arc::ptr_eq(&old, &output.facts));
        }
    }

    #[test]
    fn missing_type_references_and_explicit_binding_conflicts_are_rejected() {
        let mut output = pending();
        let mut invalid = patch("E");
        invalid.bindings[0].1.effects.push(EffectRef {
            name: "Error".into(),
            args: vec![EffectArgRef::Type(crate::TypeId(u32::MAX))],
        });
        assert!(materialize_effect_row_instantiations(&mut output, &[invalid]).is_err());
        Arc::make_mut(&mut output.facts)
            .generic_instantiations
            .get_mut(&HirExprId(3))
            .unwrap()
            .effect_row_bindings
            .push((
                "E".into(),
                EffectRowRef {
                    effects: vec![],
                    tail: Some("F".into()),
                },
            ));
        assert!(materialize_effect_row_instantiations(&mut output, &[patch("E")]).is_err());
    }
}
