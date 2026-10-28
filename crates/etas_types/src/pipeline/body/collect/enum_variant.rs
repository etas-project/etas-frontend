use crate::{
    CallableSignature, ConstraintOrigin, SymbolTypeFact, TypeConstraint, TypeId,
    pipeline::context::BodyCollectContext,
};
use etas_core::{Span, TypeDiagnosticCode};
use etas_hir::{HirFieldInit, HirGenericArg, HirRecordExpr, ResolveResult, ResolvedPath};

pub(super) fn declaration<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
) -> Option<crate::EnumVariantLayoutFact> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)?;
    let symbol = ctx.ctx.hir.symbols.get(symbol)?;
    let (owner, variant_index) = crate::facts::enums::constructor_identity(
        ctx.ctx.interner.store(),
        symbol,
        ctx.ctx.symbol_type(symbol.id)?,
    )?;
    let variant = ctx.ctx.enum_layout(owner)?.variants.get(variant_index)?;
    (variant.name == symbol.name).then(|| variant.clone())
}

pub(super) fn signature<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
) -> Option<CallableSignature> {
    declaration(ctx, path)?;
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)?;
    let SymbolTypeFact::Flow { signature } = ctx.ctx.symbol_type(symbol)?.clone() else {
        return None;
    };
    super::expr::instantiate_callable_signature(ctx, signature)
}

pub(super) fn invalid<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    span: Span,
    message: impl Into<String>,
) {
    ctx.validate(crate::ValidationRequest::Diagnostic {
        code: TypeDiagnosticCode::TypeMismatch,
        span,
        message: message.into(),
    });
}

pub(super) fn collect_nullary<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
    expected: Option<TypeId>,
) -> Option<TypeId> {
    let variant = declaration(ctx, path)?;
    if !variant.fields.is_empty() || variant.field_names.is_some() {
        return None;
    }
    let signature = signature(ctx, path)?;
    let output = signature.output;
    if let Some(expected) = expected {
        ctx.emit(TypeConstraint::Equal {
            lhs: output,
            rhs: expected,
            origin: ConstraintOrigin { span: path.span },
        });
    }
    emit_constructor(ctx, signature, Vec::new(), path.span);
    Some(output)
}

fn emit_constructor<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: CallableSignature,
    args: Vec<TypeId>,
    span: Span,
) {
    let callee = ctx
        .ctx
        .interner
        .intern(crate::Type::Function(crate::FlowType {
            input: signature.params,
            output: signature.output,
            effects: signature.effects,
        }));
    ctx.emit(TypeConstraint::Callable {
        call: None,
        callee,
        generic_params: signature.generic_params,
        generic_args: Vec::new(),
        arg_exprs: Vec::new(),
        args,
        output: signature.output,
        origin: ConstraintOrigin { span },
    });
}

pub(super) fn collect_record<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    record: &HirRecordExpr,
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let Some(path) = record.path.as_ref() else {
        return Ok(None);
    };
    let Some(variant) = declaration(ctx, path) else {
        return Ok(None);
    };
    let Some(signature) = signature(ctx, path) else {
        return Ok(None);
    };
    let Some(names) = variant.field_names else {
        invalid(
            ctx,
            record.span,
            "positional or nullary variant cannot use named-field construction",
        );
        return Ok(Some(signature.output));
    };
    if !record.generic_args.is_empty() {
        if record.generic_args.len() != signature.generic_params.len() {
            invalid(
                ctx,
                record.span,
                "enum constructor has wrong number of type arguments",
            );
        }
        for (param, arg) in signature.generic_params.iter().zip(&record.generic_args) {
            ctx.checkpoint()?;
            if let HirGenericArg::Type(ty) = arg {
                if let Some(ty) = crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty) {
                    ctx.emit(TypeConstraint::Equal {
                        lhs: param.subject,
                        rhs: ty,
                        origin: ConstraintOrigin { span: record.span },
                    });
                }
            } else {
                invalid(ctx, record.span, "enum constructor requires type arguments");
            }
        }
    }
    if let Some(expected) = expected {
        ctx.emit(TypeConstraint::Equal {
            lhs: signature.output,
            rhs: expected,
            origin: ConstraintOrigin { span: record.span },
        });
    }
    let mut seen = std::collections::HashSet::new();
    let mut arguments = vec![None; names.len()];
    for field in &record.fields {
        ctx.checkpoint()?;
        let (name, span) = match field {
            HirFieldInit::Named { name, span, .. } | HirFieldInit::Shorthand { name, span, .. } => {
                (name, *span)
            }
        };
        if !seen.insert(name) {
            invalid(ctx, span, format!("duplicate variant field `{name}`"));
        }
        let slot = names.iter().position(|candidate| candidate == name);
        let target = slot.and_then(|index| signature.params.get(index)).copied();
        if target.is_none() {
            invalid(ctx, span, format!("unknown variant field `{name}`"));
        }
        let actual = super::record::collect_field_value(ctx, field, target)?;
        if let Some(slot) = slot {
            arguments[slot] = Some(actual);
        }
    }
    for name in &names {
        ctx.checkpoint()?;
        if !seen.contains(name) {
            invalid(ctx, record.span, format!("missing variant field `{name}`"));
        }
    }
    // Field expressions are collected in source order; the callable solver receives
    // declaration-order arguments and the same generic bounds as positional calls.
    let output = signature.output;
    if let Some(args) = arguments.into_iter().collect::<Option<Vec<_>>>() {
        emit_constructor(ctx, signature, args, record.span);
    }
    Ok(Some(output))
}

pub(super) fn collect_pattern<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
    fields: &[etas_hir::HirRecordPatField],
    span: Span,
    ty: TypeId,
) -> Result<bool, E> {
    ctx.checkpoint()?;
    let Some(variant) = declaration(ctx, path) else {
        return Ok(false);
    };
    let Some(signature) = signature(ctx, path) else {
        return Ok(false);
    };
    let Some(names) = variant.field_names else {
        invalid(
            ctx,
            span,
            "positional or nullary variant cannot use a named-field pattern",
        );
        return Ok(true);
    };
    ctx.emit(TypeConstraint::Equal {
        lhs: signature.output,
        rhs: ty,
        origin: ConstraintOrigin { span },
    });
    let mut seen = std::collections::HashSet::new();
    for field in fields {
        ctx.checkpoint()?;
        if !seen.insert(&field.name) {
            invalid(ctx, field.span, "duplicate variant pattern field");
        }
        let target = names
            .iter()
            .position(|name| name == &field.name)
            .and_then(|index| signature.params.get(index))
            .copied();
        if let Some(target) = target {
            if let Some(pat) = field.pat {
                super::pattern::collect_pattern(ctx, pat, target)?;
            }
        } else {
            invalid(
                ctx,
                field.span,
                format!("unknown variant pattern field `{}`", field.name),
            );
        }
    }
    for name in &names {
        ctx.checkpoint()?;
        if !seen.contains(name) {
            invalid(ctx, span, format!("missing variant pattern field `{name}`"));
        }
    }
    Ok(true)
}
