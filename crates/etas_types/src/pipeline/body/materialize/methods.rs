use crate::{FlowType, Type, method::MethodTarget, pipeline::context::TypePipelineContext};

use super::BodyPipelineState;

pub(super) fn materialize(ctx: &mut TypePipelineContext<'_>, state: &mut BodyPipelineState) {
    for (expr, fact) in &state.solver_report.checked_method_calls {
        match &fact.target {
            MethodTarget::Standard {
                intrinsic: Some(intrinsic),
                qualified_path,
                overload,
                operation,
            } if *operation != etas_std::FlowSourceMethodOperation::CheckedIndex => {
                let callable_type = ctx.interner.intern(Type::Function(FlowType {
                    input: fact.signature.params.clone(),
                    output: fact.signature.output,
                    effects: fact.signature.effects.clone(),
                }));
                state.solver_report.std_intrinsic_calls.insert(
                    *expr,
                    crate::facts::CheckedStdCallFact {
                        qualified_path: qualified_path.clone(),
                        intrinsic: *intrinsic,
                        overload: *overload,
                        callable_type,
                        signature: fact.signature.clone(),
                    },
                );
            }
            MethodTarget::Agent { symbol, .. } => {
                state
                    .solver_report
                    .agent_method_calls
                    .insert(*expr, *symbol);
            }
            _ => {}
        }
    }
}
