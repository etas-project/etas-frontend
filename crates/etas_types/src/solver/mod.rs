pub mod assignability;
pub mod callable;
pub mod effect_row;
pub mod field;
pub mod index;
mod numeric_literal;
pub mod report;
pub mod spec_solver;
pub mod unification;

pub use assignability::{Assignable, TypeRelation};
pub use report::{SolverFailure, SolverReport};
pub use unification::{Substitution, TypeUnifier, UnifyError};

use std::collections::HashMap;
use std::collections::HashSet;

use crate::{
    ConstraintOrigin, NumericLiteralKind, PrimitiveType, SpecFacts, SpecObligation, Type,
    TypeConstraint, TypeId, TypeStore, ty::display_type,
};

pub struct TypeSolveInput<'a> {
    pub store: &'a TypeStore,
    pub spec_facts: &'a SpecFacts,
    pub constraints: &'a [TypeConstraint],
    pub spec_obligations: &'a [SpecObligation],
}

pub struct TypeSolver;

impl TypeSolver {
    pub fn solve(input: TypeSolveInput<'_>) -> SolverReport {
        let mut report = SolverReport::default();
        let mut pending_access_constraints = Vec::new();
        let mut pending_unary_constraints = Vec::new();
        let mut numeric_literals = Vec::new();
        for constraint in input.constraints {
            match constraint {
                TypeConstraint::NumericLiteral { .. } => numeric_literals.push(constraint),
                TypeConstraint::Equal { lhs, rhs, origin } => {
                    let lhs = resolve_known_substitutions(input.store, &report, *lhs);
                    let rhs = resolve_known_substitutions(input.store, &report, *rhs);
                    let mut unifier = TypeUnifier::new(input.store);
                    if unifier.unify(lhs, rhs).is_err() {
                        report.push(SolverFailure {
                            code: etas_core::TypeDiagnosticCode::TypeMismatch,
                            span: origin.span,
                            message: "type equality constraint could not be solved".to_owned(),
                        });
                    } else {
                        report.substitutions.extend(unifier.substitution());
                    }
                }
                TypeConstraint::Assignable {
                    from,
                    to,
                    origin,
                    reason,
                } => {
                    let from = resolve_known_substitutions(input.store, &report, *from);
                    let to = resolve_known_substitutions(input.store, &report, *to);
                    if !is_assignable_for_reason(input.store, from, to, reason) {
                        report.push(SolverFailure {
                            code: assignability_failure_code(reason),
                            span: origin.span,
                            message: format!(
                                "{}: `{}` is not assignable to `{}`",
                                assignability_failure_message(reason),
                                display_type(input.store, from),
                                display_type(input.store, to)
                            ),
                        });
                    } else {
                        let mut unifier = TypeUnifier::new(input.store);
                        if unifier.unify(from, to).is_ok() {
                            report.substitutions.extend(unifier.substitution());
                        }
                    }
                }
                TypeConstraint::Callable {
                    call,
                    callee,
                    generic_params,
                    generic_args,
                    arg_exprs,
                    args,
                    output,
                    origin,
                } => {
                    let callee = resolve_known_substitutions(input.store, &report, *callee);
                    let output = resolve_known_substitutions(input.store, &report, *output);
                    let args = args
                        .iter()
                        .map(|arg| resolve_known_substitutions(input.store, &report, *arg))
                        .collect::<Vec<_>>();
                    report.append(solve_callable_constraint(
                        input.store,
                        input.spec_facts,
                        CallableConstraintSolveInput {
                            call: *call,
                            callee,
                            generic_params,
                            explicit_generic_args: generic_args,
                            arg_exprs,
                            args: &args,
                            output,
                            origin: *origin,
                        },
                    ));
                }
                TypeConstraint::MethodCall {
                    method,
                    candidates,
                    generic_args,
                    args,
                    output,
                    origin,
                } => {
                    let mut solved = None;
                    for candidate in candidates {
                        let output = resolve_known_substitutions(input.store, &report, *output);
                        let args = args
                            .iter()
                            .map(|arg| resolve_known_substitutions(input.store, &report, *arg))
                            .collect::<Vec<_>>();
                        if method == "cast"
                            && candidates.iter().any(|candidate| {
                                is_checked_message_cast_candidate(input.store, candidate.ty)
                            })
                        {
                            solved = Some(solve_checked_message_cast(
                                input.store,
                                generic_args,
                                &args,
                                output,
                                *origin,
                            ));
                            break;
                        }
                        if method == "data"
                            && args
                                .iter()
                                .any(|arg| contains_secret_wrapper(input.store, *arg))
                        {
                            continue;
                        }
                        let candidate_report = solve_callable_constraint(
                            input.store,
                            input.spec_facts,
                            CallableConstraintSolveInput {
                                call: None,
                                callee: candidate.ty,
                                generic_params: &candidate.generic_params,
                                explicit_generic_args: generic_args,
                                arg_exprs: &[],
                                args: &args,
                                output,
                                origin: *origin,
                            },
                        );
                        if candidate_report.failures.is_empty() {
                            solved = Some(candidate_report);
                            break;
                        }
                    }
                    if let Some(candidate_report) = solved {
                        report.append(candidate_report);
                    } else {
                        report.push(SolverFailure {
                            code: etas_core::TypeDiagnosticCode::TypeMismatch,
                            span: origin.span,
                            message: method_call_failure_message(method, input.store, args),
                        });
                    }
                }
                TypeConstraint::FieldAccess { .. } => {
                    solve_or_defer_access_constraint(
                        input.store,
                        &mut report,
                        constraint,
                        &mut pending_access_constraints,
                    );
                }
                TypeConstraint::IndexAccess { .. } | TypeConstraint::SliceAccess { .. } => {
                    solve_or_defer_access_constraint(
                        input.store,
                        &mut report,
                        constraint,
                        &mut pending_access_constraints,
                    );
                }
                TypeConstraint::Iterable { iter, item, origin } => {
                    let iter = resolve_known_substitutions(input.store, &report, *iter);
                    let item = resolve_known_substitutions(input.store, &report, *item);
                    let iter = resolve_known_substitutions(input.store, &report, iter);
                    if let Some(iter_item) = iterable_item(input.store, iter) {
                        let mut unifier = TypeUnifier::new(input.store);
                        if unifier.unify(iter_item, item).is_ok() {
                            report.substitutions.extend(unifier.substitution());
                        } else if !assignability::is_assignable(input.store, iter_item, item) {
                            report.push(SolverFailure {
                                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                                span: origin.span,
                                message: "for-loop iterator item type does not match pattern"
                                    .to_owned(),
                            });
                        }
                    } else {
                        report.push(SolverFailure {
                            code: etas_core::TypeDiagnosticCode::TypeMismatch,
                            span: origin.span,
                            message: "for-loop iterator must be an iterable collection or range"
                                .to_owned(),
                        });
                    }
                }
                TypeConstraint::Unary { .. } => pending_unary_constraints.push(constraint),
                TypeConstraint::TryOperand {
                    operand,
                    output,
                    error: _,
                    origin,
                } => {
                    let operand = resolve_known_substitutions(input.store, &report, *operand);
                    let output = resolve_known_substitutions(input.store, &report, *output);
                    if !is_assignable_for_reason(
                        input.store,
                        operand,
                        output,
                        &crate::AssignabilityReason::Other,
                    ) {
                        report.push(SolverFailure {
                            code: etas_core::TypeDiagnosticCode::TypeMismatch,
                            span: origin.span,
                            message: "try operand value does not match Result ok type".to_owned(),
                        });
                    } else {
                        let mut unifier = TypeUnifier::new(input.store);
                        if unifier.unify(operand, output).is_ok() {
                            report.substitutions.extend(unifier.substitution());
                        }
                    }
                }
            }
        }
        solve_numeric_literals(input.store, &mut report, numeric_literals);
        solve_unary_constraints(input.store, &mut report, pending_unary_constraints);
        solve_pending_access_constraints(input.store, &mut report, pending_access_constraints);
        report.append(spec_solver::solve_spec_obligations(
            input.store,
            input.spec_facts,
            input.spec_obligations,
            &report.named_substitutions,
        ));
        report
    }
}

struct CallableConstraintSolveInput<'a> {
    call: Option<etas_hir::HirExprId>,
    callee: TypeId,
    generic_params: &'a [crate::CallableGenericParam],
    explicit_generic_args: &'a [crate::CallableGenericArg],
    arg_exprs: &'a [Option<etas_hir::HirExprId>],
    args: &'a [TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
}

fn solve_callable_constraint(
    store: &TypeStore,
    spec_facts: &SpecFacts,
    input: CallableConstraintSolveInput<'_>,
) -> SolverReport {
    let inferred_names;
    let declared_names;
    let generic_param_names = if input.generic_params.is_empty() {
        inferred_names = callable::callable_schematic_param_names(store, input.callee);
        inferred_names.as_slice()
    } else {
        declared_names = input
            .generic_params
            .iter()
            .filter(|param| param.kind == crate::CallableGenericParamKind::Type)
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        declared_names.as_slice()
    };
    let effect_param_names = input
        .generic_params
        .iter()
        .filter(|param| param.kind == crate::CallableGenericParamKind::Effect)
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    let mut type_generic_args = Vec::new();
    let mut effect_row_bindings = HashMap::new();
    let mut deferred_effect_row_bindings = HashMap::new();
    let mut generic_argument_failure = None;
    if input.explicit_generic_args.len() > input.generic_params.len() {
        generic_argument_failure = Some(format!(
            "call accepts at most {} generic argument(s), got {}",
            input.generic_params.len(),
            input.explicit_generic_args.len()
        ));
    } else {
        for (param, arg) in input.generic_params.iter().zip(input.explicit_generic_args) {
            match (param.kind, arg) {
                (crate::CallableGenericParamKind::Type, crate::CallableGenericArg::Type(ty)) => {
                    type_generic_args.push(*ty);
                }
                (
                    crate::CallableGenericParamKind::Effect,
                    crate::CallableGenericArg::EffectRow(row),
                ) => {
                    effect_row_bindings.insert(param.name.clone(), row.clone());
                }
                (crate::CallableGenericParamKind::Type, _) => {
                    generic_argument_failure = Some(format!(
                        "generic parameter `{}` requires a type argument",
                        param.name
                    ));
                    break;
                }
                (crate::CallableGenericParamKind::Effect, _) => {
                    generic_argument_failure = Some(format!(
                        "generic parameter `effect {}` requires an effect-row argument",
                        param.name
                    ));
                    break;
                }
            }
        }
    }
    if let Some(message) = generic_argument_failure {
        let mut report = SolverReport::default();
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: input.origin.span,
            message,
        });
        return report;
    }
    let mut report = callable::solve_callable_with_named_substitutions(
        store,
        callable::CallableSolveInput {
            callee: input.callee,
            generic_param_names,
            explicit_generic_args: &type_generic_args,
            args: input.args,
            output: input.output,
            origin: input.origin,
            initial_named_substitutions: HashMap::new(),
        },
    );
    if report.failures.is_empty() {
        let Some(Type::Function(flow)) = store.get(input.callee) else {
            return report;
        };
        for (index, (actual, expected)) in input.args.iter().zip(&flow.input).enumerate() {
            let inference = effect_row::infer_bindings_from_types(
                store,
                *expected,
                *actual,
                &report.named_substitutions,
                &report.substitutions,
                &effect_param_names,
                &mut effect_row_bindings,
            );
            if !inference.matches {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: input.origin.span,
                    message: "callable argument effect row does not match the generic effect-row contract"
                        .to_owned(),
                });
                break;
            }
            for name in inference.deferred {
                let Some(expr) = input.arg_exprs.get(index).copied().flatten() else {
                    report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: input.origin.span,
                        message: format!(
                            "generic effect-row parameter `effect {name}` requires a checked latent flow source"
                        ),
                    });
                    break;
                };
                if deferred_effect_row_bindings
                    .insert(name.clone(), expr)
                    .is_some_and(|existing| existing != expr)
                {
                    report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::TypeMismatch,
                        span: input.origin.span,
                        message: format!(
                            "generic effect-row parameter `effect {name}` has incompatible latent flow sources"
                        ),
                    });
                    break;
                }
            }
        }
        for name in &effect_param_names {
            if effect_row_bindings.contains_key(name)
                || deferred_effect_row_bindings.contains_key(name)
            {
                continue;
            }
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: input.origin.span,
                message: format!(
                    "generic effect-row parameter `effect {name}` could not be inferred for this call"
                ),
            });
        }
    }
    if report.failures.is_empty() {
        let mut obligations = Vec::new();
        for param in input
            .generic_params
            .iter()
            .filter(|param| param.kind == crate::CallableGenericParamKind::Type)
        {
            if param.bounds.is_empty() {
                continue;
            }
            let solved_subject = resolve_substitution(store, &report.substitutions, param.subject);
            let ty = report
                .named_substitutions
                .get(&param.name)
                .copied()
                .or_else(|| {
                    (solved_subject != param.subject
                        && !matches!(store.get(solved_subject), Some(Type::Var(_))))
                    .then_some(solved_subject)
                });
            let Some(ty) = ty else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: input.origin.span,
                    message: format!(
                        "bounded generic parameter `{}` could not be inferred for this call",
                        param.name
                    ),
                });
                continue;
            };
            obligations.extend(param.bounds.iter().map(|bound| SpecObligation {
                ty,
                spec: bound.spec.clone(),
                args: bound.args.clone(),
                span: input.origin.span,
            }));
        }
        if report.failures.is_empty() {
            report.append(spec_solver::solve_spec_obligations(
                store,
                spec_facts,
                &obligations,
                &report.named_substitutions,
            ));
        }
    }
    if report.failures.is_empty()
        && let Some(call) = input.call
    {
        let type_bindings = generic_param_names
            .iter()
            .filter_map(|name| {
                report
                    .named_substitutions
                    .get(name)
                    .copied()
                    .or_else(|| {
                        input
                            .generic_params
                            .iter()
                            .find(|param| param.name == *name)
                            .and_then(|param| {
                                let solved = resolve_substitution(
                                    store,
                                    &report.substitutions,
                                    param.subject,
                                );
                                (solved != param.subject
                                    && !matches!(store.get(solved), Some(Type::Var(_))))
                                .then_some(solved)
                            })
                    })
                    .map(|ty| (name.clone(), ty))
            })
            .collect::<Vec<_>>();
        let effect_row_bindings = effect_param_names
            .iter()
            .filter_map(|name| {
                effect_row_bindings
                    .get(name)
                    .cloned()
                    .map(|row| (name.clone(), row))
            })
            .collect::<Vec<_>>();
        let deferred_effect_row_bindings = effect_param_names
            .iter()
            .filter_map(|name| {
                deferred_effect_row_bindings
                    .get(name)
                    .copied()
                    .map(|expr| (name.clone(), expr))
            })
            .collect::<Vec<_>>();
        if !type_bindings.is_empty()
            || !effect_row_bindings.is_empty()
            || !deferred_effect_row_bindings.is_empty()
        {
            report.generic_instantiations.insert(
                call,
                crate::GenericInstantiationFact {
                    type_bindings,
                    effect_row_bindings,
                    deferred_effect_row_bindings,
                },
            );
        }
    }
    // Generic names belong to this call only. Type-variable substitutions carry
    // the solved result into the surrounding body without cross-call collisions.
    report.named_substitutions.clear();
    report
}

fn solve_unary_constraints(
    store: &TypeStore,
    report: &mut SolverReport,
    constraints: Vec<&TypeConstraint>,
) {
    for constraint in constraints {
        let TypeConstraint::Unary {
            op,
            operand,
            output,
            origin,
        } = constraint
        else {
            continue;
        };
        let operand = resolve_known_substitutions(store, report, *operand);
        let output = resolve_known_substitutions(store, report, *output);
        let valid = match op {
            etas_hir::HirUnaryOp::Not => {
                matches!(
                    store.get(operand),
                    Some(Type::Primitive(crate::PrimitiveType::Bool))
                ) && matches!(
                    store.get(output),
                    Some(Type::Primitive(crate::PrimitiveType::Bool)) | Some(Type::Var(_))
                )
            }
            etas_hir::HirUnaryOp::Neg => {
                is_numeric(store, operand) && assignability::is_assignable(store, operand, output)
            }
        };
        if !valid {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: origin.span,
                message: match op {
                    etas_hir::HirUnaryOp::Not => "logical negation operand must be bool".to_owned(),
                    etas_hir::HirUnaryOp::Neg => {
                        "unary negation operand must be numeric".to_owned()
                    }
                },
            });
        }
    }
}

fn solve_numeric_literals(
    store: &TypeStore,
    report: &mut SolverReport,
    literals: Vec<&TypeConstraint>,
) {
    let default_integer = primitive_type_id(store, PrimitiveType::I32);
    let default_float = primitive_type_id(store, PrimitiveType::F64);
    for literal in literals {
        let TypeConstraint::NumericLiteral {
            expr,
            text,
            kind,
            ty,
            origin,
        } = literal
        else {
            continue;
        };
        let mut resolved = resolve_known_substitutions(store, report, *ty);
        if matches!(store.get(resolved), Some(Type::Var(_))) {
            let default = match kind {
                NumericLiteralKind::Integer => default_integer,
                NumericLiteralKind::Float => default_float,
            };
            let Some(default) = default else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "numeric literal default type is missing from the type environment"
                        .to_owned(),
                });
                continue;
            };
            let mut unifier = TypeUnifier::new(store);
            if unifier.unify(resolved, default).is_ok() {
                report.substitutions.extend(unifier.substitution());
                resolved = default;
            }
        }
        if let Err(reason) = numeric_literal::validate(store, *kind, text, resolved) {
            let message = if expr.is_some() {
                format!(
                    "numeric literal `{text}` cannot have checked type `{}`: {reason}",
                    display_type(store, resolved),
                )
            } else {
                format!(
                    "literal pattern does not match checked type `{}`: {reason} (literal `{text}`)",
                    display_type(store, resolved),
                )
            };
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: origin.span,
                message,
            });
            continue;
        }
        if let Some(expr) = expr {
            report.inferred_expr_types.insert(*expr, resolved);
        }
    }
}

fn primitive_type_id(store: &TypeStore, primitive: PrimitiveType) -> Option<TypeId> {
    store.iter().find_map(|(id, ty)| {
        matches!(ty, Type::Primitive(found) if *found == primitive).then_some(id)
    })
}

fn solve_or_defer_access_constraint(
    store: &TypeStore,
    report: &mut SolverReport,
    constraint: &TypeConstraint,
    pending: &mut Vec<TypeConstraint>,
) {
    match solve_access_constraint(store, report, constraint) {
        AccessConstraintOutcome::Solved(solved) => report.append(*solved),
        AccessConstraintOutcome::Pending => pending.push(constraint.clone()),
    }
}

fn solve_pending_access_constraints(
    store: &TypeStore,
    report: &mut SolverReport,
    mut pending: Vec<TypeConstraint>,
) {
    while !pending.is_empty() {
        let mut next = Vec::new();
        let mut solved_any = false;
        for constraint in pending {
            match solve_access_constraint(store, report, &constraint) {
                AccessConstraintOutcome::Solved(solved) => {
                    solved_any = true;
                    report.append(*solved);
                }
                AccessConstraintOutcome::Pending => next.push(constraint),
            }
        }
        if !solved_any {
            for constraint in next {
                report.push(unresolved_access_failure(store, report, &constraint));
            }
            break;
        }
        pending = next;
    }
}

enum AccessConstraintOutcome {
    Solved(Box<SolverReport>),
    Pending,
}

fn solve_access_constraint(
    store: &TypeStore,
    report: &SolverReport,
    constraint: &TypeConstraint,
) -> AccessConstraintOutcome {
    match constraint {
        TypeConstraint::FieldAccess {
            base,
            field,
            output,
            origin,
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            if is_unresolved_type_var(store, base) {
                return AccessConstraintOutcome::Pending;
            }
            let output = resolve_known_substitutions(store, report, *output);
            AccessConstraintOutcome::Solved(Box::new(field::solve_field_access(
                store, base, field, output, *origin,
            )))
        }
        TypeConstraint::IndexAccess {
            expr,
            base,
            index,
            output,
            index_error,
            origin,
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            let index = resolve_known_substitutions(store, report, *index);
            if is_unresolved_type_var(store, base) || is_unresolved_type_var(store, index) {
                return AccessConstraintOutcome::Pending;
            }
            let output = resolve_known_substitutions(store, report, *output);
            AccessConstraintOutcome::Solved(Box::new(index::solve_index_access(
                store,
                index::IndexAccessSolveInput {
                    expr: *expr,
                    base,
                    index,
                    output,
                    index_error: *index_error,
                    origin: *origin,
                    substitutions: &report.substitutions,
                },
            )))
        }
        TypeConstraint::SliceAccess {
            expr,
            base,
            start,
            end,
            output,
            origin,
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            let start = resolve_known_substitutions(store, report, *start);
            let end = resolve_known_substitutions(store, report, *end);
            if is_unresolved_type_var(store, base)
                || is_unresolved_type_var(store, start)
                || is_unresolved_type_var(store, end)
            {
                return AccessConstraintOutcome::Pending;
            }
            let output = resolve_known_substitutions(store, report, *output);
            AccessConstraintOutcome::Solved(Box::new(index::solve_slice_access(
                store,
                index::SliceAccessSolveInput {
                    expr: *expr,
                    base,
                    start,
                    end,
                    output,
                    origin: *origin,
                    substitutions: &report.substitutions,
                },
            )))
        }
        _ => unreachable!("only access constraints can be solved by the access worklist"),
    }
}

fn is_unresolved_type_var(store: &TypeStore, ty: TypeId) -> bool {
    matches!(store.get(ty), Some(Type::Var(_)))
}

fn unresolved_access_failure(
    store: &TypeStore,
    report: &SolverReport,
    constraint: &TypeConstraint,
) -> SolverFailure {
    match constraint {
        TypeConstraint::FieldAccess {
            base,
            field,
            origin,
            ..
        } => {
            let base = resolve_known_substitutions(store, report, *base);
            SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "field access `{field}` requires a known record-like base type, but `{}` could not be inferred",
                    display_type(store, base)
                ),
            }
        }
        TypeConstraint::IndexAccess { base, origin, .. } => {
            let base = resolve_known_substitutions(store, report, *base);
            SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "index access requires a known indexable base type, but `{}` could not be inferred",
                    display_type(store, base)
                ),
            }
        }
        TypeConstraint::SliceAccess { base, origin, .. } => {
            let base = resolve_known_substitutions(store, report, *base);
            SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "slice access requires a known sliceable base type, but `{}` could not be inferred",
                    display_type(store, base)
                ),
            }
        }
        _ => unreachable!("only access constraints can remain pending"),
    }
}

fn iterable_item(store: &TypeStore, ty: TypeId) -> Option<TypeId> {
    match store.get(ty)? {
        Type::Array(inner)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Slice(inner)
        | Type::MemorySelection(inner) => Some(*inner),
        Type::Range { index } => Some(*index),
        _ => None,
    }
}

fn is_numeric(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty),
        Some(Type::IntegerLiteral { .. })
            | Some(Type::Primitive(
                crate::PrimitiveType::I8
                    | crate::PrimitiveType::I16
                    | crate::PrimitiveType::I32
                    | crate::PrimitiveType::I64
                    | crate::PrimitiveType::I128
                    | crate::PrimitiveType::ISize
                    | crate::PrimitiveType::U8
                    | crate::PrimitiveType::U16
                    | crate::PrimitiveType::U32
                    | crate::PrimitiveType::U64
                    | crate::PrimitiveType::U128
                    | crate::PrimitiveType::USize
                    | crate::PrimitiveType::F32
                    | crate::PrimitiveType::F64
            ))
    )
}

fn is_assignable_for_reason(
    store: &TypeStore,
    from: crate::TypeId,
    to: crate::TypeId,
    reason: &crate::AssignabilityReason,
) -> bool {
    if assignability::is_assignable(store, from, to) {
        return true;
    }
    if record_exact_assignable(store, from, to) {
        return true;
    }
    if matches!(reason, crate::AssignabilityReason::Pattern)
        && let Some(crate::Type::Nominal(nominal)) = store.get(from)
        && let Some(representation) = nominal.representation
    {
        return record_subset_assignable(store, representation, to)
            || assignability::is_assignable(store, representation, to);
    }
    if matches!(reason, crate::AssignabilityReason::Pattern)
        && record_subset_assignable(store, from, to)
    {
        return true;
    }
    if matches!(reason, crate::AssignabilityReason::Constructor)
        && let Some(representation) = nominal_representation(store, to)
    {
        return assignability::is_assignable(store, from, representation);
    }
    false
}

fn nominal_representation(store: &TypeStore, ty: crate::TypeId) -> Option<crate::TypeId> {
    let mut current = ty;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let (representation, _) = crate::nominal_representation_parts(store, current)?;
        current = representation;
        if crate::nominal_representation_parts(store, current).is_none() {
            return Some(current);
        }
    }
}

fn assignability_failure_code(
    reason: &crate::AssignabilityReason,
) -> etas_core::TypeDiagnosticCode {
    match reason {
        crate::AssignabilityReason::Finish => etas_core::TypeDiagnosticCode::FinishTypeMismatch,
        crate::AssignabilityReason::Branch => etas_core::TypeDiagnosticCode::BranchTypeMismatch,
        crate::AssignabilityReason::Annotation | crate::AssignabilityReason::Constructor => {
            etas_core::TypeDiagnosticCode::Mismatch
        }
        _ => etas_core::TypeDiagnosticCode::TypeMismatch,
    }
}

fn assignability_failure_message(reason: &crate::AssignabilityReason) -> String {
    match reason {
        crate::AssignabilityReason::Finish => {
            "finish value does not match handler result type".to_owned()
        }
        crate::AssignabilityReason::HandlerResume => {
            "resume payload does not match handled action return type".to_owned()
        }
        crate::AssignabilityReason::HandlerPattern => {
            "handler action pattern does not match action parameter type".to_owned()
        }
        crate::AssignabilityReason::Pattern => {
            "literal pattern does not match the matched value type".to_owned()
        }
        crate::AssignabilityReason::Branch => {
            "match or if branch type does not match the expected branch type".to_owned()
        }
        _ => "type does not match the expected type".to_owned(),
    }
}

fn method_call_failure_message(method: &str, store: &TypeStore, args: &[TypeId]) -> String {
    let method_name = match method {
        "system" | "user" | "assistant" | "data" => format!("Prompt.{method}"),
        "cast" => "Message.cast".to_owned(),
        _ => method.to_owned(),
    };
    if args.iter().any(|arg| contains_secret_wrapper(store, *arg)) {
        return format!(
            "{method_name} does not accept Secret[T] without explicit declassification"
        );
    }
    format!("{method_name} receiver or arguments do not match any standard method signature")
}

fn is_checked_message_cast_candidate(store: &TypeStore, candidate: TypeId) -> bool {
    let Some(Type::Function(flow)) = store.get(candidate) else {
        return false;
    };
    if flow.input.len() != 1 {
        return false;
    }
    matches!(store.get(flow.input[0]), Some(Type::Message(_)))
        && matches!(
            store.get(flow.output),
            Some(Type::Option(inner)) if matches!(store.get(*inner), Some(Type::Message(_)))
        )
}

fn solve_checked_message_cast(
    store: &TypeStore,
    generic_args: &[crate::CallableGenericArg],
    args: &[TypeId],
    output: TypeId,
    origin: ConstraintOrigin,
) -> SolverReport {
    let mut report = SolverReport::default();
    if generic_args.len() != 1 {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span: origin.span,
            message: format!(
                "Message.cast expects 1 type argument, got {}",
                generic_args.len()
            ),
        });
        return report;
    }
    if args.len() != 1 {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span: origin.span,
            message: format!(
                "Message.cast expects 0 argument(s), got {}",
                args.len().saturating_sub(1)
            ),
        });
        return report;
    }
    if message_inner_type(store, args[0]).is_none() {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message:
                "Message.cast receiver or arguments do not match any standard method signature"
                    .to_owned(),
        });
        return report;
    }
    let crate::CallableGenericArg::Type(payload) = generic_args[0] else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "Message.cast requires a type generic argument".to_owned(),
        });
        return report;
    };
    if !is_option_message_of(store, output, payload) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "Message.cast result type does not match requested payload type".to_owned(),
        });
    }
    report
}

fn message_inner_type(store: &TypeStore, ty: TypeId) -> Option<TypeId> {
    match store.get(ty) {
        Some(Type::Message(inner)) => Some(*inner),
        Some(Type::Applied { constructor, args }) if args.len() == 1 => matches!(
            store.get(TypeId(constructor.0)),
            Some(Type::Nominal(nominal)) if nominal.name == "std.agent.message.Message"
        )
        .then_some(args[0]),
        _ => None,
    }
}

fn is_option_message_of(store: &TypeStore, ty: TypeId, payload: TypeId) -> bool {
    let Some(Type::Option(inner)) = store.get(ty) else {
        return false;
    };
    message_inner_type(store, *inner).is_some_and(|actual| actual == payload)
}

fn contains_secret_wrapper(store: &TypeStore, ty: TypeId) -> bool {
    match store.get(ty) {
        Some(Type::Trust {
            wrapper: crate::TrustWrapper::Secret,
            ..
        }) => true,
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner })
        | Some(Type::Trust { inner, .. }) => contains_secret_wrapper(store, *inner),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_secret_wrapper(store, *key) || contains_secret_wrapper(store, *value)
        }
        Some(Type::Result { ok, err }) => {
            contains_secret_wrapper(store, *ok) || contains_secret_wrapper(store, *err)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_secret_wrapper(store, *element)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_secret_wrapper(store, *input))
                || contains_secret_wrapper(store, flow.output)
        }
        _ => false,
    }
}

fn resolve_substitution(store: &TypeStore, substitutions: &Substitution, ty: TypeId) -> TypeId {
    let mut current = ty;
    let mut seen = 0usize;
    while let Some(Type::Var(var)) = store.get(current) {
        let Some(next) = substitutions.get(*var) else {
            break;
        };
        if next == current || seen > 64 {
            break;
        }
        current = next;
        seen += 1;
    }
    current
}

fn resolve_known_substitutions(store: &TypeStore, report: &SolverReport, ty: TypeId) -> TypeId {
    let mut current = resolve_substitution(store, &report.substitutions, ty);
    let mut seen = 0usize;
    while let Some(Type::Named(name)) = store.get(current) {
        let Some(next) = report.named_substitutions.get(&name.name).copied() else {
            break;
        };
        if next == current || seen > 64 {
            break;
        }
        current = resolve_substitution(store, &report.substitutions, next);
        seen += 1;
    }
    current
}

fn record_exact_assignable(store: &TypeStore, from: crate::TypeId, to: crate::TypeId) -> bool {
    let Some(from_record) = record_fields(store, from) else {
        return false;
    };
    let Some(to_record) = record_fields(store, to) else {
        return false;
    };
    from_record.fields.len() == to_record.fields.len() && record_subset_assignable(store, from, to)
}

fn record_subset_assignable(store: &TypeStore, from: crate::TypeId, to: crate::TypeId) -> bool {
    let Some(from_record) = record_fields(store, from) else {
        return false;
    };
    let Some(to_record) = record_fields(store, to) else {
        return false;
    };
    to_record.fields.iter().all(|expected| {
        from_record
            .fields
            .iter()
            .find(|candidate| candidate.name == expected.name)
            .is_some_and(|actual| {
                is_assignable_for_reason(
                    store,
                    actual.ty,
                    expected.ty,
                    &crate::AssignabilityReason::Pattern,
                )
            })
    })
}

fn record_fields(store: &TypeStore, ty: crate::TypeId) -> Option<crate::RecordType> {
    match store.get(ty) {
        Some(crate::Type::Record(record)) => Some(record.clone()),
        _ => None,
    }
}
