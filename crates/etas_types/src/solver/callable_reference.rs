use std::collections::HashMap;

use etas_core::TypeDiagnosticCode;

use super::{SolverFailure, SolverReport, effect_row, resolve_known_substitutions};
use crate::{CallableGenericParamKind, GenericInstantiationFact, TypeConstraint, TypeStore};

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
