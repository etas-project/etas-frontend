use crate::{SpecFacts, TypeSolver, solver::TypeSolveInput};

use super::state::BodyPipelineState;

pub fn run(
    state: &mut BodyPipelineState,
    interner: &mut crate::TypeInterner,
    spec_facts: &SpecFacts,
    span: etas_core::Span,
) {
    // Pattern/call collection can introduce generic aggregate types after field
    // hints ran. Their projections must exist before the read-only solver.
    if let Err(error) = crate::ty::materialize_representations(
        interner,
        state
            .provisional
            .expr_types
            .values()
            .copied()
            .chain(state.provisional.stmt_types.values().copied())
            .chain(state.provisional.pat_types.values().copied()),
    ) {
        state.solver_report.push(crate::solver::SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            message: format!("solver type representation could not be materialized: {error}"),
        });
        return;
    }
    state.solver_report = TypeSolver::solve(TypeSolveInput {
        constraints: &state.constraints,
        spec_obligations: &state.spec_obligations,
        spec_facts,
        store: interner.store(),
    });
}
