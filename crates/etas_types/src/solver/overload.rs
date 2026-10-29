use super::*;

pub(super) struct Invocation<'a> {
    pub expr: etas_hir::HirExprId,
    pub callee: TypeId,
    pub candidates: &'a [crate::CallableCandidate],
    pub probes: &'a [TypeId],
    pub generic_args: &'a [crate::CallableGenericArg],
    pub arg_exprs: &'a [Option<etas_hir::HirExprId>],
    pub args: &'a [TypeId],
    pub output: TypeId,
    pub origin: ConstraintOrigin,
}

pub(super) fn solve<E>(
    input: &TypeSolveInput<'_>,
    call: Invocation<'_>,
    report: &mut SolverReport,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    if call.probes.len() != call.candidates.len() || call.candidates.is_empty() {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: call.origin.span,
            message: "overloaded call is missing its generated candidate constraints".into(),
        });
        return Ok(());
    }
    let args = call
        .args
        .iter()
        .map(|ty| resolve_known_substitutions(input.store, report, *ty))
        .collect::<Vec<_>>();
    let mut selected = None;
    // Probe with independent result variables: an expected result must never
    // select an overload or recover an erased descriptor's generic contract.
    for (candidate, probe) in call.candidates.iter().zip(call.probes) {
        checkpoint()?;
        let solved = solve_callable_constraint(
            input.store,
            input.spec_facts,
            CallableConstraintSolveInput {
                rigid_type_parameters: input.rigid_type_parameters,
                call: Some(call.expr),
                callee: candidate.ty,
                generic_params: &candidate.signature.generic_params,
                explicit_generic_args: call.generic_args,
                arg_exprs: call.arg_exprs,
                args: &args,
                output: *probe,
                origin: call.origin,
                known_substitutions: report.substitutions.clone(),
            },
        );
        if solved.failures.is_empty() {
            if selected.is_some() {
                report.push(SolverFailure { code: etas_core::TypeDiagnosticCode::TypeMismatch, span: call.origin.span, message: "call input types match more than one overload; supply unambiguous arguments".into() });
                return Ok(());
            }
            selected = Some(candidate);
        }
    }
    let Some(candidate) = selected else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: call.origin.span,
            message: format!(
                "call input types ({}) do not match any registered overload",
                args.iter()
                    .map(|ty| display_type(input.store, *ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
        return Ok(());
    };
    let mut solved = solve_callable_constraint(
        input.store,
        input.spec_facts,
        CallableConstraintSolveInput {
            rigid_type_parameters: input.rigid_type_parameters,
            call: Some(call.expr),
            callee: candidate.ty,
            generic_params: &candidate.signature.generic_params,
            explicit_generic_args: call.generic_args,
            arg_exprs: call.arg_exprs,
            args: &args,
            output: resolve_known_substitutions(input.store, report, call.output),
            origin: call.origin,
            known_substitutions: report.substitutions.clone(),
        },
    );
    if solved.failures.is_empty() {
        let mut unifier = TypeUnifier::with_known_substitution(input.store, &solved.substitutions);
        if unifier.unify(call.callee, candidate.ty).is_err() {
            solved.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: call.origin.span,
                message: "selected overload conflicts with its checked callee type".into(),
            });
            report.append(solved);
            return Ok(());
        }
        solved.substitutions.extend(&unifier.into_substitution());
        if let crate::CallableCandidateOperation::StdIntrinsic {
            qualified_path,
            intrinsic,
            overload,
            ..
        } = &candidate.operation
        {
            solved.std_intrinsic_calls.insert(
                call.expr,
                crate::facts::CheckedStdCallFact {
                    qualified_path: qualified_path.clone(),
                    intrinsic: *intrinsic,
                    overload: *overload,
                    callable_type: candidate.ty,
                    signature: candidate.signature.clone(),
                },
            );
        }
        if let crate::CallableCandidateOperation::EffectAction {
            symbol,
            target,
            overload,
        } = candidate.operation
        {
            solved.action_calls.insert(
                call.expr,
                crate::facts::CheckedActionCallFact {
                    action_symbol: symbol,
                    target,
                    overload,
                    signature: candidate.signature.clone(),
                },
            );
        }
    }
    report.append(solved);
    Ok(())
}
