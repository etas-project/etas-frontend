use etas_core::{Span, TypeDiagnosticCode};

use crate::{
    SymbolTypeFact, Type, TypeConstructorId, TypeId, ValidationRequest,
    pipeline::context::BodyCollectContext,
};

pub(super) fn target<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    fact: SymbolTypeFact,
    arguments: &[TypeId],
    span: Span,
) -> Option<TypeId> {
    let ty = match fact {
        SymbolTypeFact::NominalType {
            constructor,
            params,
            ..
        } => {
            let arguments = type_arguments(ctx, &params, arguments, span)?;
            apply(ctx, constructor, arguments)
        }
        SymbolTypeFact::Type { constructor } => {
            let params = ctx
                .ctx
                .type_generics(TypeId(constructor.0))
                .map(|params| {
                    params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let arguments = type_arguments(ctx, &params, arguments, span)?;
            apply(ctx, constructor, arguments)
        }
        SymbolTypeFact::TypeAlias { target, params } => {
            let names = params
                .iter()
                .map(|param| param.name.clone())
                .collect::<Vec<_>>();
            let arguments = type_arguments(ctx, &names, arguments, span)?;
            require_parameter_bounds(ctx, params, arguments.clone(), span);
            let substitutions = names.into_iter().zip(arguments).collect();
            match crate::substitute_named_params(&mut ctx.ctx.interner, target, &substitutions) {
                Ok(ty) => ty,
                Err(error) => {
                    invalid(
                        ctx,
                        span,
                        TypeDiagnosticCode::IncompleteTypeFacts,
                        format!("constructor alias substitution failed: {error}"),
                    );
                    return None;
                }
            }
        }
        _ => return None,
    };
    require_bounds(ctx, ty, span);
    Some(ty)
}

fn type_arguments<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    parameters: &[String],
    arguments: &[TypeId],
    span: Span,
) -> Option<Vec<TypeId>> {
    if arguments.is_empty() {
        return Some(parameters.iter().map(|_| ctx.fresh_type_var()).collect());
    }
    if parameters.len() != arguments.len() {
        invalid(
            ctx,
            span,
            TypeDiagnosticCode::ArityMismatch,
            format!(
                "type constructor expects {} type arguments, got {}",
                parameters.len(),
                arguments.len()
            ),
        );
        return None;
    }
    Some(arguments.to_vec())
}

fn apply<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    constructor: TypeConstructorId,
    args: Vec<TypeId>,
) -> TypeId {
    if args.is_empty() {
        TypeId(constructor.0)
    } else {
        ctx.ctx.interner.intern(Type::Applied { constructor, args })
    }
}

fn require_bounds<E>(ctx: &mut BodyCollectContext<'_, '_, E>, target: TypeId, span: Span) {
    let Some(Type::Applied { constructor, args }) = ctx.ctx.interner.store().get(target).cloned()
    else {
        return;
    };
    let Some(parameters) = ctx.ctx.type_generics(TypeId(constructor.0)).cloned() else {
        invalid(
            ctx,
            span,
            TypeDiagnosticCode::IncompleteTypeFacts,
            "generic constructor is missing checked type parameter facts",
        );
        return;
    };
    require_parameter_bounds(ctx, parameters, args, span);
}

fn require_parameter_bounds<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    parameters: Vec<crate::CallableGenericParam>,
    args: Vec<TypeId>,
    span: Span,
) {
    match crate::constraint::generic_bounds::instantiate(
        &mut ctx.ctx.interner,
        &parameters,
        &args,
        span,
    ) {
        Ok(obligations) => ctx.state.spec_obligations.extend(obligations),
        Err(validation) => ctx.validate(validation),
    }
}

fn invalid<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    span: Span,
    code: TypeDiagnosticCode,
    message: impl Into<String>,
) {
    ctx.validate(ValidationRequest::Diagnostic {
        code,
        span,
        message: message.into(),
    });
}
