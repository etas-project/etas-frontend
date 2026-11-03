use super::*;

#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    pub expr: &'a etas_hir::HirExprId,
    pub method: &'a str,
    pub access_module: &'a Option<etas_hir::HirModuleId>,
    pub candidates: &'a [crate::CallableCandidate],
    pub generic_args: &'a [crate::CallableGenericArg],
    pub arg_exprs: &'a [Option<etas_hir::HirExprId>],
    pub args: &'a [TypeId],
    pub output: &'a TypeId,
    pub origin: &'a ConstraintOrigin,
}

impl Input<'_> {
    fn has_receiver(&self) -> bool {
        !self.candidates.iter().any(|candidate| {
            matches!(
                candidate.operation,
                crate::CallableCandidateOperation::StdIntrinsic {
                    has_receiver: false,
                    ..
                }
            )
        })
    }

    pub fn ready(&self, store: &TypeStore, report: &SolverReport) -> bool {
        !self.has_receiver()
            || self.args.first().is_none_or(|receiver| {
                !is_unresolved_type_var(
                    store,
                    resolve_known_substitutions(store, report, *receiver),
                )
            })
    }
}

pub(super) fn solve<E>(
    input: &TypeSolveInput<'_>,
    invocation: Input<'_>,
    report: &mut SolverReport,
    checked_method_constraints: &mut Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let Input {
        expr,
        method,
        access_module,
        candidates,
        generic_args,
        arg_exprs,
        args,
        output,
        origin,
    } = invocation;
    let actual = args
        .iter()
        .map(|arg| resolve_known_substitutions(input.store, report, *arg))
        .collect::<Vec<_>>();
    if let Some(receiver) = actual.first().filter(|_| invocation.has_receiver()) {
        match agent::resolve_method(
            input.store,
            input.declaration_facts,
            *receiver,
            method,
            *access_module,
            *origin,
        ) {
            Ok(Some(method)) => {
                let Some((candidate, callable)) =
                    candidates
                        .iter()
                        .find_map(|candidate| match candidate.operation {
                            crate::CallableCandidateOperation::AgentMethod { symbol, callable }
                                if symbol == method.symbol =>
                            {
                                Some((candidate, callable))
                            }
                            _ => None,
                        })
                else {
                    report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: origin.span,
                        message: "selected agent method has no generated callable constraints"
                            .into(),
                    });
                    return Ok(());
                };
                let Some((_, arguments)) = arg_exprs
                    .split_first()
                    .filter(|_| arg_exprs.len() == actual.len())
                else {
                    report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: origin.span,
                        message: "method call is missing its checked receiver/argument identities"
                            .into(),
                    });
                    return Ok(());
                };
                let output = resolve_known_substitutions(input.store, report, *output);
                let mut solved = solve_callable_constraint(
                    input.store,
                    input.spec_facts,
                    CallableConstraintSolveInput {
                        rigid_type_parameters: input.rigid_type_parameters,
                        call: Some(*expr),
                        callee: callable,
                        generic_params: &candidate.signature.generic_params,
                        explicit_generic_args: generic_args,
                        arg_exprs: arguments,
                        args: &actual[1..],
                        output,
                        origin: *origin,
                        known_substitutions: report.substitutions.clone(),
                    },
                );
                if solved.failures.is_empty() {
                    solved.agent_method_calls.insert(*expr, method.symbol);
                }
                report.append(solved);
                return Ok(());
            }
            Err(failure) => {
                report.push(failure);
                return Ok(());
            }
            Ok(None) => {}
        }
    }
    let mut solved = None;
    let mut single_failure = None;
    for candidate in candidates {
        checkpoint()?;
        if matches!(
            candidate.operation,
            crate::CallableCandidateOperation::AgentMethod { .. }
        ) {
            continue;
        }
        let output = resolve_known_substitutions(input.store, report, *output);
        let args = args
            .iter()
            .map(|arg| resolve_known_substitutions(input.store, report, *arg))
            .collect::<Vec<_>>();
        if method == "cast"
            && candidates
                .iter()
                .any(|candidate| is_checked_message_cast_candidate(input.store, candidate.ty))
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
        let mut candidate_report = solve_callable_constraint(
            input.store,
            input.spec_facts,
            CallableConstraintSolveInput {
                rigid_type_parameters: input.rigid_type_parameters,
                call: Some(*expr),
                callee: candidate.ty,
                generic_params: &candidate.signature.generic_params,
                explicit_generic_args: generic_args,
                arg_exprs,
                args: &args,
                output,
                origin: *origin,
                known_substitutions: report.substitutions.clone(),
            },
        );
        if candidate_report.failures.is_empty() {
            if let crate::CallableCandidateOperation::StdIntrinsic {
                qualified_path,
                intrinsic,
                overload,
                ..
            } = &candidate.operation
            {
                candidate_report.std_intrinsic_calls.insert(
                    *expr,
                    crate::facts::CheckedStdCallFact {
                        qualified_path: qualified_path.clone(),
                        intrinsic: *intrinsic,
                        overload: *overload,
                        callable_type: candidate.ty,
                        signature: candidate.signature.clone(),
                    },
                );
            }
            if let crate::CallableCandidateOperation::CheckedIndex { error } = &candidate.operation
            {
                if let ([base, index], Some(error)) = (args.as_slice(), error) {
                    checked_method_constraints.push(TypeConstraint::IndexAccess {
                        expr: *expr,
                        base: *base,
                        index: *index,
                        output,
                        index_error: Some(*error),
                        origin: *origin,
                    });
                } else {
                    candidate_report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: origin.span,
                        message: "checked index method requires receiver/index arguments and the canonical IndexError fact".to_owned(),
                    });
                }
            }
            solved = Some(candidate_report);
            break;
        } else if candidates.len() == 1 && !invocation.has_receiver() {
            single_failure = Some(candidate_report.failures);
        }
    }
    if let Some(candidate_report) = solved {
        report.append(candidate_report);
    } else if let Some(failures) = single_failure {
        report.failures.extend(failures);
    } else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: method_call_failure_message(method, input.store, args),
        });
    }
    Ok(())
}

pub(super) fn solve_pending<E>(
    input: &TypeSolveInput<'_>,
    report: &mut SolverReport,
    mut pending: Vec<Input<'_>>,
    access: &mut Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    solve_ready(input, report, &mut pending, access, checkpoint)?;
    for method in pending {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: method.origin.span,
            message: format!(
                "method `{}` requires a resolved receiver value type",
                method.method
            ),
        });
    }
    Ok(())
}

pub(super) fn solve_ready<E>(
    input: &TypeSolveInput<'_>,
    report: &mut SolverReport,
    pending: &mut Vec<Input<'_>>,
    access: &mut Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    loop {
        checkpoint()?;
        let mut progress = solve_ready_access_constraints(
            input.store,
            input.known_std_types,
            input.spec_facts,
            report,
            access,
            checkpoint,
        )?;
        let mut next = Vec::new();
        let mut generated = Vec::new();
        for method in std::mem::take(pending) {
            checkpoint()?;
            if method.ready(input.store, report) {
                solve(input, method, report, &mut generated, checkpoint)?;
                progress = true;
            } else {
                next.push(method);
            }
        }
        access.extend(generated);
        *pending = next;
        if !progress {
            break;
        }
    }
    Ok(())
}
