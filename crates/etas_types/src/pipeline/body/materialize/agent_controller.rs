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
        let bindings = HashMap::from([("C".into(), context), ("O".into(), inference.output)]);
        let step = operation(ctx, etas_std::intrinsic::runtime::AGENT_STEP, &bindings)?;
        let invoke = operation(ctx, etas_std::intrinsic::runtime::TOOL_INVOKE, &bindings)?;
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
    intrinsic: u32,
    bindings: &HashMap<String, TypeId>,
) -> Result<CheckedAgentOperationFact, String> {
    let registry = ctx.std_registry.clone();
    let descriptor = registry
        .intrinsic(etas_std::StdIntrinsicId(intrinsic))
        .ok_or("agent controller operation is missing its canonical intrinsic")?;
    let symbol = registry
        .lookup_qualified(&descriptor.qualified_path)
        .ok_or("agent controller operation is missing its standard declaration")?;
    let etas_std::StdDecl::Flow(flow) = &symbol.decl else {
        return Err("agent controller operation is not a standard flow".into());
    };
    let scope = registry
        .module(symbol.module)
        .map(|module| module.path.as_slice());
    let mut signature = crate::lower::std::lower_std_flow_signature(ctx, &registry, flow, scope);
    super::substitute_callable_signature(
        ctx,
        &crate::Substitution::default(),
        bindings,
        &mut signature,
    )
    .map_err(|error| error.to_string())?;
    signature.generic_params.clear();
    Ok(CheckedAgentOperationFact {
        target: symbol.id,
        signature,
    })
}
