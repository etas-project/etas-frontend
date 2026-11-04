use std::collections::{HashMap, HashSet};

use crate::{CheckedSpecBound, CheckedSpecRef, SpecFacts, Type, TypeId, TypeStore};

/// The caller's declared evidence, not implementations loaded elsewhere in the project.
pub(crate) fn arguments(
    store: &TypeStore,
    facts: &SpecFacts,
    receiver: TypeId,
    target: &CheckedSpecRef,
) -> Result<Vec<Vec<TypeId>>, String> {
    let bounds = match store.get(receiver) {
        Some(Type::Existential { bounds }) => bounds.clone(),
        Some(Type::Named(name)) if name.source_parameter.is_some() => facts
            .type_param_bounds
            .get(&name.source_parameter.unwrap())
            .into_iter()
            .flatten()
            .map(|bound| CheckedSpecBound {
                spec: CheckedSpecRef::Source(bound.spec_symbol),
                args: bound.args.clone(),
            })
            .collect(),
        _ => return Ok(Vec::new()),
    };
    let target = super::external::normalize(facts, target.clone());
    Ok(closure(store, facts, bounds)?
        .into_iter()
        .filter(|bound| bound.spec == target)
        .map(|bound| bound.args)
        .collect())
}

pub(crate) fn closure(
    store: &TypeStore,
    facts: &SpecFacts,
    mut pending: Vec<CheckedSpecBound>,
) -> Result<Vec<CheckedSpecBound>, String> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    while let Some(bound) = pending.pop() {
        let spec = super::external::normalize(facts, bound.spec);
        if !visited.insert((spec.clone(), bound.args.clone())) {
            continue;
        }
        result.push(CheckedSpecBound {
            spec: spec.clone(),
            args: bound.args.clone(),
        });
        let (names, parameters, supers) = match &spec {
            CheckedSpecRef::Source(symbol) => {
                let signature = facts
                    .signatures
                    .get(symbol)
                    .ok_or("bound has no checked spec declaration")?;
                (
                    signature.param_names.as_slice(),
                    signature.params.as_slice(),
                    signature
                        .super_specs
                        .iter()
                        .map(|bound| CheckedSpecBound {
                            spec: bound.super_spec.clone(),
                            args: bound.args.clone(),
                        })
                        .collect::<Vec<_>>(),
                )
            }
            CheckedSpecRef::External(identity) => {
                let signature = facts
                    .external_signatures
                    .get(identity)
                    .ok_or("bound has no checked external spec declaration")?;
                (
                    signature.param_names.as_slice(),
                    &[][..],
                    signature.super_specs.clone(),
                )
            }
            CheckedSpecRef::Canonical(key) => {
                let signature = facts
                    .canonical
                    .signatures
                    .get(key)
                    .ok_or("bound has no canonical spec declaration")?;
                (
                    signature.param_names.as_slice(),
                    &[][..],
                    signature.super_specs.clone(),
                )
            }
            CheckedSpecRef::Std(_) => continue,
        };
        if names.len() != bound.args.len() {
            return Err("bound evidence has inconsistent generic arity".into());
        }
        let names = names
            .iter()
            .cloned()
            .zip(bound.args.iter().copied())
            .collect::<HashMap<_, _>>();
        let parameters = parameters
            .iter()
            .copied()
            .zip(bound.args.iter().copied())
            .collect::<HashMap<_, _>>();
        let canonical = if let CheckedSpecRef::Canonical(owner) = &spec {
            bound
                .args
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    let index =
                        u32::try_from(index).map_err(|_| "spec parameter index overflow")?;
                    crate::ty::CanonicalTypeParameterIdentity::new(owner.clone(), index)
                        .map(|identity| (identity, *arg))
                        .map_err(|_| "invalid canonical spec parameter")
                })
                .collect::<Result<HashMap<_, _>, _>>()?
        } else {
            HashMap::new()
        };
        let mut substitution = crate::TypeSubstitutionEngine::existing(store, &names)
            .with_source_parameters(&parameters)
            .with_canonical_parameters(&canonical);
        for mut parent in supers {
            parent.args = parent
                .args
                .iter()
                .map(|ty| substitution.substitute(*ty))
                .collect::<Result<_, _>>()
                .map_err(|error| error.to_string())?;
            pending.push(parent);
        }
    }
    Ok(result)
}
