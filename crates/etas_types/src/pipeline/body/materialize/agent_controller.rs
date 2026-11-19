use std::collections::HashMap;

use crate::{
    TypeId,
    facts::inference::{CheckedAgentControllerFact, CheckedAgentOperationFact},
    pipeline::{body::state::BodyPipelineState, context::TypePipelineContext},
};

pub(super) fn materialize(
    ctx: &mut TypePipelineContext<'_>,
    state: &BodyPipelineState,
) -> Result<HashMap<etas_hir::HirExprId, CheckedAgentControllerFact>, String> {
    let mut result = HashMap::new();
    for (expr, inference) in &state.solver_report.agent_inferences {
        let context = ctx
            .agent_method(inference.method)
            .ok_or("inference controller has no checked agent method")?
            .context_type;
        let step = operation(ctx, &["std", "agent", "step"], &[context, inference.output])?;
        let invoke = operation(ctx, &["std", "tool", "invoke"], &[context])?;
        crate::ty::materialize_representations(
            &mut ctx.interner,
            step.signature
                .params
                .iter()
                .chain(&invoke.signature.params)
                .copied()
                .chain([step.signature.output, invoke.signature.output]),
        )
        .map_err(|error| error.to_string())?;
        result.insert(
            *expr,
            CheckedAgentControllerFact {
                context,
                step,
                invoke,
            },
        );
    }
    Ok(result)
}

fn operation(
    ctx: &mut TypePipelineContext<'_>,
    path: &[&str],
    arguments: &[TypeId],
) -> Result<CheckedAgentOperationFact, String> {
    let registry = ctx.std_registry.clone();
    let symbol = registry
        .lookup_qualified(path)
        .ok_or("agent controller operation is missing its standard declaration")?;
    let etas_std::StdDecl::Flow(flow) = &symbol.decl else {
        return Err("agent controller operation is not a standard flow".into());
    };
    let scope = registry
        .module(symbol.module)
        .map(|module| module.path.as_slice());
    let mut signature = crate::lower::std::lower_std_flow_signature(ctx, &registry, flow, scope);
    if signature.generic_params.len() != arguments.len() {
        return Err(format!(
            "{} has an incompatible controller signature",
            path.join(".")
        ));
    }
    let bindings = signature
        .generic_params
        .iter()
        .zip(arguments)
        .map(|(parameter, argument)| (parameter.subject, *argument))
        .collect();
    let names = HashMap::new();
    let mut engine = crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &names, None)
        .with_type_parameters(&bindings);
    crate::facts::methods::map_signature(&mut signature, &mut |ty| engine.substitute(ty))
        .map_err(|error| error.to_string())?;
    signature.generic_params.clear();
    Ok(CheckedAgentOperationFact {
        target: symbol.id,
        signature,
    })
}
