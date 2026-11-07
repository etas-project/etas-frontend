use crate::{FlowType, Type, method::MethodTarget, pipeline::context::TypePipelineContext};

use super::BodyPipelineState;

pub(super) fn materialize(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), String> {
    let bound_values = state
        .solver_report
        .bound_method_values
        .keys()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for (expr, fact) in state.solver_report.checked_method_calls.iter_mut().chain(
        state
            .solver_report
            .bound_method_values
            .iter_mut()
            .map(|(expr, value)| (expr, &mut value.method)),
    ) {
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
            let output = if bound_values.contains(expr) {
                let crate::method::MethodReceiver::Value { slot } = fact.receiver else {
                    unreachable!()
                };
                let mut input = fact.signature.params.clone();
                input.remove(slot as usize);
                ctx.interner.intern(Type::Function(FlowType {
                    input,
                    output: fact.signature.output,
                    effects: fact.signature.effects.clone(),
                }))
            } else {
                fact.signature.output
            };
            state
                .solver_report
                .inferred_expr_types
                .insert(*expr, output);
        }
        match &fact.target {
            MethodTarget::Standard {
                intrinsic: Some(intrinsic),
                qualified_path,
                overload,
                ..
            } => {
                let mut runtime_signature = fact.signature.clone();
                if let crate::method::MethodReceiver::Value { slot } = fact.receiver {
                    let receiver = runtime_signature
                        .params
                        .get_mut(slot as usize)
                        .ok_or("checked standard method receiver slot is out of bounds")?;
                    *receiver = fact.receiver_type;
                }
                let callable_type = ctx.interner.intern(Type::Function(FlowType {
                    input: runtime_signature.params.clone(),
                    output: runtime_signature.output,
                    effects: runtime_signature.effects.clone(),
                }));
                state.solver_report.std_intrinsic_calls.insert(
                    *expr,
                    crate::facts::CheckedStdCallFact {
                        qualified_path: qualified_path.clone(),
                        intrinsic: *intrinsic,
                        overload: *overload,
                        callable_type,
                        signature: runtime_signature,
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
