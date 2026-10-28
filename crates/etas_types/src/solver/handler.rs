use crate::{ConstraintOrigin, Type, TypeId, TypeStore, TypeVarId};

use super::{SolverFailure, SolverReport, TypeUnifier, resolve_known_substitutions};

pub(super) struct PendingParameter {
    name: String,
    variable: TypeId,
    rigid: TypeId,
    instances: Vec<TypeId>,
    scope_start: TypeVarId,
    origin: ConstraintOrigin,
}

pub(super) fn finalize_parameters(
    store: &TypeStore,
    report: &mut SolverReport,
    pending: Vec<PendingParameter>,
) {
    for parameter in pending {
        if is_parametric(store, report, parameter.variable, parameter.scope_start) {
            bind(
                store,
                report,
                parameter.variable,
                parameter.rigid,
                &parameter.name,
                parameter.origin,
            );
            continue;
        }
        let candidate = resolve_known_substitutions(store, report, parameter.variable);
        let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
        if parameter.instances.is_empty()
            || parameter.instances.iter().any(|actual| {
                let actual = resolve_known_substitutions(store, report, *actual);
                unifier.unify(actual, candidate).is_err()
            })
        {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: parameter.origin.span,
                message: format!(
                    "handler resume payload does not match every request instantiation of `{}`",
                    parameter.name
                ),
            });
        } else {
            report.substitutions.extend(&unifier.into_substitution());
        }
    }
    // A still-unbound handler-owned variable proves uniformity. Equal spelling
    // of unrelated declaration binders is not evidence of parametric behavior.
    let deferred = std::mem::take(&mut report.deferred_handler_instantiations);
    for (arm, mut fact) in deferred {
        let scope_start = parameter_scope(store, fact.parameters.iter().map(|p| p.candidate));
        for parameter in &mut fact.parameters {
            parameter.parametric = scope_start
                .is_some_and(|start| is_parametric(store, report, parameter.candidate, start));
            if parameter.parametric {
                bind(
                    store,
                    report,
                    parameter.candidate,
                    parameter.rigid,
                    &parameter.parameter,
                    ConstraintOrigin { span: fact.span },
                );
            }
        }
        report.deferred_handler_instantiations.insert(arm, fact);
    }
}

fn parameter_scope(
    store: &TypeStore,
    parameters: impl Iterator<Item = TypeId>,
) -> Option<TypeVarId> {
    parameters
        .filter_map(|ty| match store.get(ty) {
            Some(Type::Var(variable)) => Some(*variable),
            _ => None,
        })
        .min_by_key(|variable| variable.0)
}

fn is_parametric(
    store: &TypeStore,
    report: &SolverReport,
    variable: TypeId,
    start: TypeVarId,
) -> bool {
    let resolved = resolve_known_substitutions(store, report, variable);
    let Some(Type::Var(representative)) = store.get(resolved) else {
        return false;
    };
    representative.0 >= start.0
        && !report.substitutions.iter().any(|(variable, ty)| {
            variable.0 < start.0 && resolve_known_substitutions(store, report, ty) == resolved
        })
}

fn bind(
    store: &TypeStore,
    report: &mut SolverReport,
    variable: TypeId,
    target: TypeId,
    name: &str,
    origin: ConstraintOrigin,
) {
    let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
    match unifier.unify(variable, target) {
        Ok(()) => report.substitutions.extend(&unifier.into_substitution()),
        Err(error) => report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: format!(
                "handler request parameter `{name}` could not be instantiated: {error:?}"
            ),
        }),
    }
}

pub(super) fn instantiate(
    store: &TypeStore,
    report: &mut SolverReport,
    parameters: &[(String, TypeId, TypeId)],
    requests: &[etas_hir::HirExprId],
    origin: ConstraintOrigin,
    pending: &mut Vec<PendingParameter>,
) {
    let scope_start = parameter_scope(store, parameters.iter().map(|(_, variable, _)| *variable));
    for (name, variable, rigid) in parameters {
        let mut instances = Vec::new();
        for request in requests {
            let binding = report
                .generic_instantiations
                .get(request)
                .and_then(|fact| {
                    fact.type_bindings
                        .iter()
                        .find(|(parameter, _)| parameter == name)
                })
                .map(|(_, ty)| resolve_known_substitutions(store, report, *ty));
            match binding {
                Some(ty) => instances.push(ty),
                None => report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: format!("handled request {request:?} has no checked instantiation for action parameter `{name}`"),
                }),
            }
        }
        if let Some(target) = instances.first().copied().filter(|first| {
            instances.len() == requests.len() && instances.iter().all(|ty| ty == first)
        }) {
            bind(store, report, *variable, target, name, origin);
        } else if let Some(scope_start) = scope_start {
            pending.push(PendingParameter {
                name: name.clone(),
                variable: *variable,
                rigid: *rigid,
                instances,
                scope_start,
                origin,
            });
        } else {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: "handler parameter has no inference-variable identity".into(),
            });
        }
    }
}
