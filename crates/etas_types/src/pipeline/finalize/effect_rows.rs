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
    }
    // Validate the whole batch before publishing any completed checked facts.
    output.store = Arc::new(interner.into_store());
    let facts = Arc::make_mut(&mut output.facts);
    facts.expr_types.extend(source_types);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DeferredEffectRowObligation, EffectRef, GenericInstantiationFact, PrimitiveType,
        TypeInterner,
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
