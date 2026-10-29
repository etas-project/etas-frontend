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

pub(super) fn select_request_signature(
    store: &TypeStore,
    report: &mut SolverReport,
    candidates: &[crate::constraint::ty::HandlerActionCandidate],
    requests: &[etas_hir::HirExprId],
    origin: ConstraintOrigin,
) -> Option<usize> {
    let fail = |report: &mut SolverReport, code, message: String| {
        report.push(SolverFailure {
            code,
            span: origin.span,
            message,
        });
    };
    if candidates.is_empty() {
        fail(
            report,
            etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            "handler overload has no checked request input signature".into(),
        );
        return None;
    }
    if requests.is_empty() {
        fail(report, etas_core::TypeDiagnosticCode::TypeMismatch, "handler resume payload cannot be specialized without a matching request input signature".into());
        return None;
    }
    let mut selected = None;
    for request in requests {
        let signature = report
            .action_calls
            .get(request)
            .map(|fact| &fact.signature)
            .or_else(|| {
                report
                    .std_intrinsic_calls
                    .get(request)
                    .map(|fact| &fact.signature)
            });
        let Some(signature) = signature else {
            fail(
                report,
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                format!("handled request {request:?} has no selected action input signature"),
            );
            return None;
        };
        let matching = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                if candidate.signature.params.len() != signature.params.len() {
                    return None;
                }
                let mut probe = TypeUnifier::with_known_substitution(store, &report.substitutions);
                candidate
                    .signature
                    .params
                    .iter()
                    .zip(&signature.params)
                    .all(|(expected, actual)| probe.unify(*expected, *actual).is_ok())
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let [index] = matching.as_slice() else {
            fail(
                report,
                etas_core::TypeDiagnosticCode::TypeMismatch,
                "handler request input does not identify exactly one registered action overload"
                    .into(),
            );
            return None;
        };
        if selected.is_some_and(|selected| selected != *index) {
            fail(report, etas_core::TypeDiagnosticCode::TypeMismatch, "handler requests require different action input overloads; one arm cannot assume a single payload ABI".into());
            return None;
        }
        selected = Some(*index);
    }
    selected
}

pub(super) fn bind_signature(
    store: &TypeStore,
    report: &mut SolverReport,
    signature: &crate::EffectActionSignature,
    params: &[TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
) {
    let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
    if signature.params.len() != params.len()
        || !signature
            .params
            .iter()
            .zip(params)
            .all(|(expected, actual)| unifier.unify(*expected, *actual).is_ok())
        || unifier.unify(signature.output, output).is_err()
    {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "handler payload or resume type conflicts with its checked request signature"
                .into(),
        });
        return;
    }
    report.substitutions.extend(&unifier.into_substitution());
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
