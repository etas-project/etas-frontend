use std::collections::HashMap;

use etas_core::TypeDiagnosticCode;

use super::{SolverFailure, SolverReport, effect_row, resolve_known_substitutions};
use crate::{CallableGenericParamKind, GenericInstantiationFact, TypeConstraint, TypeStore};

/// A generic value must have a monomorphic application in its lexical type
/// environment. Source type parameters are valid; solver variables are not.
pub(super) fn validate_bindings<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    constraints: &[&TypeConstraint],
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let mut failures = Vec::new();
    for constraint in constraints {
        checkpoint()?;
        let TypeConstraint::CallableReference { expr, origin, .. } = constraint else {
            continue;
        };
        let Some(fact) = report.generic_instantiations.get(expr) else {
            continue;
        };
        let mut pending = fact
            .type_bindings
            .iter()
            .map(|(_, ty)| *ty)
            .collect::<Vec<_>>();
        for (_, row) in &fact.effect_row_bindings {
            extend_row_types(&mut pending, row);
        }
        let mut seen = std::collections::HashSet::new();
        let mut incomplete = false;
        while let Some(ty) = pending.pop() {
            checkpoint()?;
            if !seen.insert(ty) {
                continue;
            }
            match store.get(ty) {
                Some(crate::Type::Var(var)) => {
                    let mut current = *var;
                    let mut chain = std::collections::HashSet::new();
                    loop {
                        checkpoint()?;
                        if !chain.insert(current) {
                            incomplete = true;
                            break;
                        }
                        let Some(next) = report.substitutions.get(current) else {
                            incomplete = true;
                            break;
                        };
                        match store.get(next) {
                            Some(crate::Type::Var(var)) => current = *var,
                            Some(_) => {
                                pending.push(next);
                                break;
                            }
                            None => {
                                incomplete = true;
                                break;
                            }
                        }
                    }
                }
                Some(
                    crate::Type::Array(inner)
                    | crate::Type::List(inner)
                    | crate::Type::Set(inner)
                    | crate::Type::Range { index: inner }
                    | crate::Type::Slice(inner)
                    | crate::Type::Option(inner)
                    | crate::Type::Refined { base: inner, .. }
                    | crate::Type::Trust { inner, .. }
                    | crate::Type::Schema(inner)
                    | crate::Type::Message(inner)
                    | crate::Type::MemorySelection(inner)
                    | crate::Type::MemoryRegion(inner),
                ) => pending.push(*inner),
                Some(crate::Type::Map { key, value } | crate::Type::Store { key, value }) => {
                    pending.extend([*key, *value]);
                }
                Some(crate::Type::Result { ok, err }) => pending.extend([*ok, *err]),
                Some(crate::Type::Record(record)) => {
                    pending.extend(record.fields.iter().map(|field| field.ty));
                }
                Some(crate::Type::Tuple(elements)) => pending.extend(elements.iter().copied()),
                Some(crate::Type::Function(flow)) => {
                    pending.extend(flow.input.iter().copied());
                    pending.push(flow.output);
                    if let Some(row) = &flow.effects {
                        extend_row_types(&mut pending, row);
                    }
                }
                Some(crate::Type::Handler(handler)) => {
                    extend_row_types(&mut pending, &handler.handled);
                    if let crate::HandlerProducedEffects::Explicit(row) = &handler.produced {
                        extend_row_types(&mut pending, row);
                    }
                    pending.extend(handler.result);
                }
                Some(crate::Type::Applied { args, .. }) => pending.extend(args.iter().copied()),
                Some(crate::Type::ResourceHandle(handle)) => match handle {
                    crate::ResourceHandleType::MemoryRegion { schema } => pending.push(*schema),
                    crate::ResourceHandleType::ExternalTool { signature } => {
                        pending.push(*signature)
                    }
                    crate::ResourceHandleType::Other { args, .. } => {
                        pending.extend(args.iter().copied())
                    }
                },
                Some(
                    crate::Type::Primitive(_)
                    | crate::Type::IntegerLiteral { .. }
                    | crate::Type::Enum(_)
                    | crate::Type::Named(_)
                    | crate::Type::Nominal(_)
                    | crate::Type::Prompt
                    | crate::Type::PromptPart
                    | crate::Type::MemoryPlace(_),
                ) => {}
                None => incomplete = true,
            }
        }
        if incomplete {
            failures.push(SolverFailure {
                code: TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: "generic callable value has an unresolved type argument".into(),
            });
        }
    }
    report.failures.extend(failures);
    Ok(())
}

fn extend_row_types(pending: &mut Vec<crate::TypeId>, row: &crate::EffectRowRef) {
    pending.extend(
        row.effects
            .iter()
            .flat_map(|effect| &effect.args)
            .filter_map(|arg| match arg {
                crate::EffectArgRef::Type(ty) => Some(*ty),
                _ => None,
            }),
    );
}

pub(super) fn solve<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    constraints: &[&TypeConstraint],
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    for constraint in constraints {
        checkpoint()?;
        let TypeConstraint::CallableReference {
            expr,
            signature,
            generic_params,
            expected,
            lexical_effect_params,
            origin,
        } = constraint
        else {
            continue;
        };
        let type_bindings = generic_params
            .iter()
            .filter(|param| param.kind == CallableGenericParamKind::Type)
            .map(|param| {
                (
                    param.name.clone(),
                    resolve_known_substitutions(store, report, param.subject),
                )
            })
            .collect::<Vec<_>>();
        let effect_params = generic_params
            .iter()
            .filter(|param| param.kind == CallableGenericParamKind::Effect)
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        let mut effect_bindings = HashMap::new();
        if !effect_params.is_empty() {
            let Some(expected) = expected else {
                report.push(SolverFailure {
                    code: TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "row-polymorphic callable value requires an expected function type to infer its effect arguments".into(),
                });
                continue;
            };
            let types = type_bindings.iter().cloned().collect();
            let inferred = effect_row::infer_scoped_bindings_from_types(
                store,
                *signature,
                *expected,
                &types,
                &report.substitutions,
                effect_row::EffectRowParameterScopes {
                    inferred: &effect_params,
                    lexical: lexical_effect_params,
                },
                &mut effect_bindings,
            );
            if !inferred.matches {
                report.push(SolverFailure {
                    code: TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message:
                        "callable value effect arguments do not match the expected function type"
                            .into(),
                });
                continue;
            }
            if !inferred.deferred.is_empty()
                || effect_params
                    .iter()
                    .any(|name| !effect_bindings.contains_key(name))
            {
                report.push(SolverFailure {
                    code: TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "callable value effect-row arguments remain unresolved; a self-binding is not an instantiation".into(),
                });
                continue;
            }
        }
        report.generic_instantiations.insert(
            *expr,
            GenericInstantiationFact {
                type_bindings,
                effect_row_bindings: effect_params
                    .into_iter()
                    .filter_map(|name| effect_bindings.remove(&name).map(|row| (name, row)))
                    .collect(),
                deferred_effect_row_obligations: Vec::new(),
            },
        );
    }
    Ok(())
}
