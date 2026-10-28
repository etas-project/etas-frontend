use std::collections::HashMap;

use crate::{
    CallableGenericArg, CallableGenericParam, CallableGenericParamKind, CallableSignature, Type,
    TypeId, TypeSubstitutionEngine, TypeSubstitutionError, ValidationRequest,
    pipeline::context::BodyCollectContext,
};
use etas_core::TypeDiagnosticCode;

pub(super) fn argument_hints<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: TypeId,
    parameters: &[CallableGenericParam],
    arguments: &[CallableGenericArg],
    span: etas_core::Span,
) -> Option<Vec<TypeId>> {
    signature_hints(ctx, callee, parameters, arguments, span).map(|flow| flow.input)
}

pub(super) fn signature_hints<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: TypeId,
    parameters: &[CallableGenericParam],
    arguments: &[CallableGenericArg],
    span: etas_core::Span,
) -> Option<crate::FlowType> {
    let flow = match ctx.ctx.interner.store().get(callee).cloned() {
        Some(Type::Function(flow)) => flow,
        Some(Type::Nominal(_) | Type::Applied { .. }) => {
            match crate::applied_representation(&mut ctx.ctx.interner, callee) {
                Ok(Some(input)) => crate::FlowType {
                    input: vec![input],
                    output: callee,
                    effects: None,
                },
                Ok(None) => return None,
                Err(error) => {
                    ctx.validate(ValidationRequest::Diagnostic {
                        code: TypeDiagnosticCode::IncompleteTypeFacts,
                        span,
                        message: format!("constructor input projection failed: {error}"),
                    });
                    return None;
                }
            }
        }
        _ => return None,
    };
    let mut types = HashMap::new();
    let mut rows = HashMap::new();
    let mut variables = crate::solver::Substitution::default();
    for (parameter, argument) in parameters.iter().zip(arguments) {
        match (parameter.kind, argument) {
            (CallableGenericParamKind::Type, CallableGenericArg::Type(ty)) => {
                // Fresh callee variables must not also replace identically named
                // caller parameters inside an argument such as Array<T>.
                if let Some(Type::Var(var)) = ctx.ctx.interner.store().get(parameter.subject) {
                    variables.insert(*var, *ty);
                } else {
                    types.insert(parameter.name.clone(), *ty);
                }
            }
            (CallableGenericParamKind::Effect, CallableGenericArg::EffectRow(row)) => {
                rows.insert(parameter.name.clone(), row.clone());
            }
            _ => {}
        }
    }
    let mut engine =
        TypeSubstitutionEngine::materializing(&mut ctx.ctx.interner, &types, Some(&variables))
            .with_effect_arguments(&rows);
    let result = (|| -> Result<crate::FlowType, TypeSubstitutionError> {
        Ok(crate::FlowType {
            input: flow
                .input
                .into_iter()
                .map(|ty| engine.substitute(ty))
                .collect::<Result<_, _>>()?,
            output: engine.substitute(flow.output)?,
            effects: flow
                .effects
                .map(|row| engine.substitute_effect_row(row))
                .transpose()?,
        })
    })();
    match result {
        Ok(signature) => Some(signature),
        Err(error) => {
            ctx.validate(ValidationRequest::Diagnostic {
                code: TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("callable argument hint substitution failed: {error}"),
            });
            None
        }
    }
}

pub(super) fn instantiate_signature<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    mut signature: CallableSignature,
) -> Option<CallableSignature> {
    let bindings = signature
        .generic_params
        .iter()
        .filter(|param| param.kind == CallableGenericParamKind::Type)
        .map(|param| (param.name.clone(), ctx.fresh_type_var()))
        .collect::<HashMap<_, _>>();
    let result = (|| -> Result<_, TypeSubstitutionError> {
        let mut engine =
            TypeSubstitutionEngine::materializing(&mut ctx.ctx.interner, &bindings, None);
        for param in &mut signature.params {
            *param = engine.substitute(*param)?;
        }
        signature.output = engine.substitute(signature.output)?;
        for param in &mut signature.generic_params {
            param.subject = engine.substitute(param.subject)?;
            for bound in &mut param.bounds {
                for arg in &mut bound.args {
                    *arg = engine.substitute(*arg)?;
                }
            }
        }
        if let Some(row) = &mut signature.effects {
            *row = engine.substitute_effect_row(row.clone())?;
        }
        if let Some(row) = &mut signature.requested_actions {
            *row = engine.substitute_effect_row(row.clone())?;
        }
        Ok(signature)
    })();
    match result {
        Ok(signature) => Some(signature),
        Err(error) => {
            ctx.validate(ValidationRequest::Diagnostic {
                code: TypeDiagnosticCode::IncompleteTypeFacts,
                span: ctx.ctx.hir.items[ctx.state.item].span(),
                message: format!("callable signature instantiation failed: {error}"),
            });
            None
        }
    }
}
