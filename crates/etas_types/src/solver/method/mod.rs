use super::*;

mod candidates;
mod instantiate;
mod operation;
mod select;

#[derive(Clone, Copy)]
pub(super) struct Input<'a> {
    pub expr: &'a etas_hir::HirExprId,
    pub binding: &'a Option<crate::facts::BoundMethodReceiver>,
    pub method: &'a str,
    pub selection: &'a crate::method::MethodSelection,
    pub argument_names: &'a [Option<String>],
    pub probes: &'a [TypeId],
    pub access_module: &'a Option<etas_hir::HirModuleId>,
    pub candidates: &'a [crate::CallableCandidate],
    pub generic_args: &'a [crate::CallableGenericArg],
    pub arg_exprs: &'a [Option<etas_hir::HirExprId>],
    pub args: &'a [TypeId],
    pub output: &'a TypeId,
    pub origin: &'a ConstraintOrigin,
}

impl Input<'_> {
    pub fn ready(&self, store: &TypeStore, report: &SolverReport) -> bool {
        self.args.first().is_some_and(|receiver| {
            !is_unresolved_type_var(store, resolve_known_substitutions(store, report, *receiver))
        })
    }
}

pub(super) fn solve<E>(
    input: &TypeSolveInput<'_>,
    invocation: Input<'_>,
    report: &mut SolverReport,
    access: &mut Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    select::solve(input, invocation, report, access, checkpoint)
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
