use std::sync::atomic::{AtomicUsize, Ordering};

use etas_core::{SourceId, Span, TextRange};
use etas_utils::{PipelineExecutionControl, PipelineInterruption, PipelineInterruptionKind};

use super::*;

struct InterruptAfter {
    remaining: AtomicUsize,
    kind: PipelineInterruptionKind,
}

impl PipelineExecutionControl for InterruptAfter {
    fn checkpoint(&self) -> Result<(), PipelineInterruption> {
        if self.remaining.fetch_sub(1, Ordering::Relaxed) == 1 {
            Err(PipelineInterruption {
                kind: self.kind,
                reason: "solver interrupted".into(),
            })
        } else {
            Ok(())
        }
    }
}

fn solve(
    store: &TypeStore,
    constraints: &[TypeConstraint],
    control: &dyn PipelineExecutionControl,
) -> Result<SolverReport, PipelineInterruption> {
    TypeSolver::solve_controlled(
        TypeSolveInput {
            rigid_type_parameters: &[],
            store,
            constraints,
            known_std_types: &Default::default(),
            declaration_facts: &Default::default(),
            spec_facts: &Default::default(),
            spec_obligations: &[],
        },
        control,
    )
}

#[test]
fn interruptions_abort_the_constraint_loop_without_a_partial_solver_report() {
    let mut types = crate::TypeInterner::new();
    let ty = types.primitive(PrimitiveType::I32);
    let constraints = vec![
        TypeConstraint::Equal {
            lhs: ty,
            rhs: ty,
            origin: ConstraintOrigin {
                span: Span::new(SourceId(7), TextRange::default())
            },
        };
        8
    ];
    for kind in [
        PipelineInterruptionKind::Cancelled,
        PipelineInterruptionKind::ResourceLimit,
    ] {
        let control = InterruptAfter {
            remaining: AtomicUsize::new(4),
            kind,
        };
        let interruption = solve(types.store(), &constraints, &control).unwrap_err();
        assert_eq!(interruption.kind, kind);
        assert_eq!(control.remaining.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn deferred_access_worklist_checks_control_inside_its_retry_loop() {
    let mut types = crate::TypeInterner::new();
    let base = types.intern(Type::Var(crate::TypeVarId(0)));
    let output = types.primitive(PrimitiveType::I32);
    let constraints = vec![
        TypeConstraint::FieldAccess {
            base,
            field: "field".into(),
            output,
            origin: ConstraintOrigin {
                span: Span::new(SourceId(9), TextRange::default())
            },
        };
        8
    ];
    // The first nine checkpoints precede deferred solving: entry plus eight
    // constraints. Interruption must therefore come from the access worklist.
    let control = InterruptAfter {
        remaining: AtomicUsize::new(11),
        kind: PipelineInterruptionKind::Cancelled,
    };
    let interruption = solve(types.store(), &constraints, &control).unwrap_err();
    assert_eq!(interruption.reason, "solver interrupted");
    assert_eq!(control.remaining.load(Ordering::Relaxed), 0);
}
