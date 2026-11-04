use super::*;

pub(super) fn pack(
    input: &TypeSolveInput<'_>,
    report: &mut SolverReport,
    constraint: &TypeConstraint,
) {
    let TypeConstraint::ExistentialPack {
        expr,
        from,
        to,
        origin,
    } = constraint
    else {
        unreachable!()
    };
    let source = resolve_known_substitutions(input.store, report, *from);
    if matches!(
        input.store.get(source),
        Some(Type::Primitive(PrimitiveType::Never))
    ) {
        return;
    }
    let Type::Existential { bounds } = input
        .store
        .get(*to)
        .expect("packing target is a checked type")
    else {
        unreachable!()
    };
    let obligations = bounds
        .iter()
        .map(|bound| SpecObligation {
            ty: source,
            spec: bound.spec.clone(),
            args: bound
                .args
                .iter()
                .map(|ty| resolve_known_substitutions(input.store, report, *ty))
                .collect(),
            span: origin.span,
        })
        .collect::<Vec<_>>();
    let proofs = spec_solver::solve_spec_obligations(
        input.store,
        input.spec_facts,
        &obligations,
        &report.named_substitutions,
    );
    if proofs.is_solved() {
        let witnesses = match witnesses(input, source, bounds) {
            Ok(witnesses) => witnesses,
            Err(message) => {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message,
                });
                return;
            }
        };
        report.existential_packs.insert(
            *expr,
            crate::facts::existentials::CheckedExistentialPackFact {
                source_type: source,
                object_type: *to,
                witnesses,
            },
        );
    }
    report.append(proofs);
}

fn witnesses(
    input: &TypeSolveInput<'_>,
    source: TypeId,
    bounds: &[crate::CheckedSpecBound],
) -> Result<Vec<crate::facts::existentials::ExistentialMethodWitnessFact>, String> {
    let symbolic = matches!(input.store.get(source), Some(Type::Existential { .. }))
        || matches!(input.store.get(source), Some(Type::Named(named)) if named.source_parameter.is_some());
    let closure =
        spec_solver::bound_evidence::closure(input.store, input.spec_facts, bounds.to_vec())?;
    let mut result = Vec::new();
    for bound in closure {
        let methods = match &bound.spec {
            crate::CheckedSpecRef::Source(symbol) => input
                .spec_facts
                .signatures
                .get(symbol)
                .map(|signature| signature.methods.as_slice())
                .ok_or("existential witness has no source spec declaration")?,
            crate::CheckedSpecRef::External(identity) => input
                .spec_facts
                .external_signatures
                .get(identity)
                .map(|signature| signature.methods.as_slice())
                .ok_or("existential witness has no external spec declaration")?,
            crate::CheckedSpecRef::Std(_) => continue,
            crate::CheckedSpecRef::Canonical(_) => {
                return Err("existential witness requires canonical member correspondence".into());
            }
        };
        for method in methods {
            let target = if symbolic {
                crate::method::MethodTarget::BoundSpec {
                    spec: bound.spec.clone(),
                    member: method.identity.clone(),
                    receiver_parameter: method.receiver_param,
                }
            } else {
                input
                    .declaration_facts
                    .methods
                    .resolve_witness(
                        input.store,
                        input.spec_facts,
                        source,
                        &bound.args,
                        &method.identity,
                    )?
                    .ok_or_else(|| {
                        format!(
                            "existential packing has no unique checked witness for `{}`",
                            method.name
                        )
                    })?
                    .declaration
                    .target
                    .clone()
            };
            result.push(crate::facts::existentials::ExistentialMethodWitnessFact {
                member: method.identity.clone(),
                bound: bound.clone(),
                target,
            });
        }
    }
    Ok(result)
}
