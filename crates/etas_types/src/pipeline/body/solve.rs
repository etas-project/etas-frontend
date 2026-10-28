use crate::{SpecFacts, TypeSolver, solver::TypeSolveInput};

use super::state::BodyPipelineState;

pub(super) fn run<E>(
    state: &mut BodyPipelineState,
    interner: &mut crate::TypeInterner,
    spec_facts: &SpecFacts,
    declaration_facts: &crate::TypeFacts,
    span: etas_core::Span,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    checkpoint()?;
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
        return Ok(());
    }
    state.solver_report = TypeSolver::solve_with_checkpoint(
        TypeSolveInput {
            rigid_type_parameters: declaration_facts
                .item_signatures
                .get(&state.item)
                .and_then(|signature| match signature {
                    crate::ItemSignature::Flow(signature)
                    | crate::ItemSignature::Tool(signature) => Some(signature),
                    crate::ItemSignature::Agent(signature) => signature.callable(),
                    crate::ItemSignature::TopLevelLet(_) => None,
                })
                .map_or(&[], |signature| signature.generic_params.as_slice()),
            constraints: &state.constraints,
            spec_obligations: &state.spec_obligations,
            spec_facts,
            store: interner.store(),
            known_std_types: &declaration_facts.known_std_types,
            declaration_facts,
        },
        checkpoint,
    )?;
    Ok(())
}
