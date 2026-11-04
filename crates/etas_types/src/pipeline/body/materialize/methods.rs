use crate::{FlowType, Type, method::MethodTarget, pipeline::context::TypePipelineContext};

use super::BodyPipelineState;

pub(super) fn materialize(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), String> {
    for (expr, fact) in &mut state.solver_report.checked_method_calls {
        if fact
            .instantiation
            .deferred_effect_row_obligations
            .is_empty()
        {
            let declaration = ctx
                .methods()
                .declaration(&fact.target, &fact.evidence)
                .ok_or("checked method has no unique declaration")?
                .clone();
            fact.signature =
                crate::method::instantiate::signature(&mut ctx.interner, &declaration, fact)?;
            state
                .solver_report
                .inferred_expr_types
                .insert(*expr, fact.signature.output);
        }
        match &fact.target {
            MethodTarget::Standard {
                intrinsic: Some(intrinsic),
                qualified_path,
                overload,
                ..
            } => {
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
    Ok(())
}
