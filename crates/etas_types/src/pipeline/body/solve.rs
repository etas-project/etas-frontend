use crate::{SpecFacts, TypeSolver, solver::TypeSolveInput};

use super::state::BodyPipelineState;

pub fn run(state: &mut BodyPipelineState, store: &crate::TypeStore, spec_facts: &SpecFacts) {
    state.solver_report = TypeSolver::solve(TypeSolveInput {
        constraints: &state.constraints,
        spec_obligations: &state.spec_obligations,
        spec_facts,
        store,
    });
}
