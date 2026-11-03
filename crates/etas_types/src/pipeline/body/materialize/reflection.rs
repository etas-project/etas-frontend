use etas_hir::{HirArg, HirExpr, HirExprId};

use crate::{
    Type, TypeId,
    facts::reflection::*,
    pipeline::{body::state::BodyPipelineState, context::TypePipelineContext},
};

pub(super) fn materialize(
    ctx: &TypePipelineContext<'_>,
    state: &BodyPipelineState,
) -> Result<std::collections::HashMap<HirExprId, CheckedDefinitionOperation>, String> {
    let compile = etas_std::intrinsic::runtime::CODE_COMPILE;
    let with_body = etas_std::intrinsic::runtime::CODE_WITH_BODY;
    let mut recipes = Vec::new();
    for (expr, fact) in &state.solver_report.std_intrinsic_calls {
        let operation = if fact.intrinsic == compile {
            DefinitionOperation::Compile
        } else if fact.intrinsic == with_body {
            DefinitionOperation::WithBody
        } else {
            continue;
        };
        recipes.push((
            *expr,
            operation,
            DefinitionOperationSelection::Intrinsic {
                intrinsic: fact.intrinsic,
                overload: fact.overload,
            },
            &fact.signature,
        ));
    }
    let compile_action = ctx
        .std_registry
        .intrinsic(etas_std::StdIntrinsicId(compile))
        .and_then(|descriptor| match descriptor.lowering {
            etas_std::LoweringHint::ForwardAction { stable_id } => Some(stable_id),
            _ => None,
        });
    for (expr, fact) in &state.solver_report.action_calls {
        let Some(etas_std::StdDecl::EffectAction(action)) = ctx
            .std_registry
            .symbol(fact.target)
            .map(|symbol| &symbol.decl)
        else {
            return Err("checked action selection has no standard declaration".into());
        };
        if action.stable_id != compile_action || compile_action.is_none() {
            continue;
        }
        recipes.push((
            *expr,
            DefinitionOperation::Compile,
            DefinitionOperationSelection::Action {
                target: fact.target,
                overload: fact.overload,
            },
            &fact.signature,
        ));
    }
    let mut output = std::collections::HashMap::new();
    for (expr, operation, selection, signature) in recipes {
        if !state
            .solver_report
            .generic_instantiations
            .contains_key(&expr)
        {
            return Err(
                "checked definition operation has no solved generic/row instantiation".into(),
            );
        }
        let input = descriptor_input(ctx, expr)
            .ok_or("checked definition operation has no descriptor argument")?;
        let descriptor_type = signature
            .params
            .first()
            .ok_or("checked definition operation has no descriptor parameter")?;
        let (family, contract_type) = descriptor_contract(ctx, *descriptor_type)
            .ok_or("checked definition operation requires a typed descriptor contract")?;
        output.insert(
            expr,
            CheckedDefinitionOperation {
                operation,
                selection,
                input,
                family,
                contract_type,
                instantiation: expr,
            },
        );
    }
    Ok(output)
}

fn descriptor_contract(
    ctx: &TypePipelineContext<'_>,
    ty: TypeId,
) -> Option<(DefinitionFamily, TypeId)> {
    let Type::Applied { constructor, args } = ctx.interner.store().get(ty)? else {
        return None;
    };
    let [contract] = args.as_slice() else {
        return None;
    };
    let code = &ctx
        .signature_base
        .map_or(&ctx.signature_facts, |base| &base.facts)
        .known_std_types
        .code;
    if code.callable_definition == Some(TypeId(constructor.0)) {
        Some((DefinitionFamily::Callable, *contract))
    } else if code.agent_definition == Some(TypeId(constructor.0)) {
        Some((DefinitionFamily::Agent, *contract))
    } else {
        None
    }
}

fn descriptor_input(ctx: &TypePipelineContext<'_>, expr: HirExprId) -> Option<HirExprId> {
    match ctx.hir.exprs.get(expr)? {
        HirExpr::Call { args, .. } | HirExpr::Perform { args, .. } => {
            args.first().map(|arg| match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            })
        }
        HirExpr::MethodCall { receiver, .. } => Some(*receiver),
        _ => None,
    }
}
