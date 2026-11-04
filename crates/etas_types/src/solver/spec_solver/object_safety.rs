use std::collections::HashSet;

use crate::{CheckedSpecBound, CheckedSpecRef, SpecKind, SpecMethodFact, TypeFacts, TypeStore};

pub(crate) fn check(
    store: &TypeStore,
    facts: &TypeFacts,
    registry: &etas_std::StdRegistry,
    bounds: &[CheckedSpecBound],
) -> Result<(), String> {
    let mut pending = bounds.to_vec();
    let mut visited = HashSet::new();
    while let Some(bound) = pending.pop() {
        let spec = match bound.spec {
            CheckedSpecRef::Source(symbol) => facts
                .external_specs
                .aliases
                .get(&symbol)
                .cloned()
                .map(CheckedSpecRef::External)
                .unwrap_or(CheckedSpecRef::Source(symbol)),
            spec => spec,
        };
        if !visited.insert((spec.clone(), bound.args.clone())) {
            continue;
        }
        let (kind, arity, methods, supers) = match &spec {
            CheckedSpecRef::Source(symbol) => {
                let signature = facts
                    .spec_signatures
                    .get(symbol)
                    .ok_or("existential bound has no checked spec signature")?;
                (
                    signature.kind,
                    signature.param_names.len(),
                    signature.methods.as_slice(),
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
                    .external_specs
                    .signatures
                    .get(identity)
                    .ok_or("existential bound has no checked external spec signature")?;
                (
                    signature.kind,
                    signature.generic_params.len(),
                    signature.methods.as_slice(),
                    signature.super_specs.clone(),
                )
            }
            CheckedSpecRef::Std(path) => {
                let Some(etas_std::StdSymbol {
                    decl: etas_std::StdDecl::Type(decl),
                    ..
                }) = registry.lookup_qualified(path)
                else {
                    return Err("existential bound has no standard spec declaration".into());
                };
                if decl.kind != etas_std::TypeDeclKind::Spec
                    || decl.params.len() != bound.args.len()
                {
                    return Err("existential standard bound has incompatible kind or arity".into());
                }
                continue;
            }
            CheckedSpecRef::Canonical(_) => {
                return Err("existential bound requires its checked spec declaration".into());
            }
        };
        if kind != SpecKind::TypeSpec {
            return Err(
                "existential objects require type specs, not callable or trace specs".into(),
            );
        }
        if arity != bound.args.len() {
            return Err("existential spec type argument arity differs from its declaration".into());
        }
        for method in methods {
            check_method(store, facts, method)?;
        }
        pending.extend(supers);
    }
    Ok(())
}

fn check_method(
    store: &TypeStore,
    facts: &TypeFacts,
    method: &SpecMethodFact,
) -> Result<(), String> {
    let signature = method
        .signature_in(facts)
        .ok_or("existential spec method has no checked signature")?;
    if signature.generic_params.len() != usize::from(method.receiver_param.is_some()) {
        return Err(format!(
            "spec method `{}` is not object-safe: generic method parameters are not allowed",
            method.name
        ));
    }
    let Some(receiver) = method.receiver_param else {
        return Ok(());
    };
    let receiver = signature
        .generic_params
        .get(receiver as usize)
        .ok_or("existential spec receiver binder is missing")?
        .subject;
    if signature.params.first() != Some(&receiver) {
        return Err("existential spec receiver does not match its checked signature".into());
    }
    let mut exposed = signature.params.iter().skip(1).copied().collect::<Vec<_>>();
    exposed.push(signature.output);
    for row in [&signature.effects, &signature.requested_actions]
        .into_iter()
        .flatten()
    {
        exposed.extend(
            row.effects
                .iter()
                .flat_map(|effect| effect.args.iter())
                .filter_map(|arg| match arg {
                    crate::EffectArgRef::Type(ty) => Some(*ty),
                    _ => None,
                }),
        );
    }
    for ty in exposed {
        if crate::ty::type_contains_id(store, ty, receiver).map_err(|error| error.to_string())? {
            return Err(format!(
                "spec method `{}` is not object-safe: its signature exposes the hidden receiver type",
                method.name
            ));
        }
    }
    Ok(())
}
