use crate::{
    CallableSignature, ConstraintOrigin, SymbolTypeFact, TypeConstraint, TypeId,
    pipeline::context::BodyCollectContext,
};
use etas_core::{Span, TypeDiagnosticCode};
use etas_hir::{
    HirFieldInit, HirGenericArg, HirItem, HirRecordExpr, ResolveResult, ResolvedPath, SymbolDef,
};

pub(super) fn declaration(
    ctx: &BodyCollectContext<'_, '_>,
    path: &ResolvedPath,
) -> Option<etas_hir::HirEnumVariant> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)?;
    let SymbolDef::EnumVariant {
        enum_item,
        variant_index,
    } = ctx.ctx.hir.symbols.get(symbol)?.def
    else {
        return None;
    };
    let HirItem::Enum(decl) = ctx.ctx.hir.items.get(enum_item)? else {
        return None;
    };
    decl.variants.get(variant_index as usize).cloned()
}

pub(super) fn signature(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &ResolvedPath,
) -> Option<CallableSignature> {
    let variant = declaration(ctx, path)?;
    let SymbolTypeFact::Flow { signature } = ctx
        .ctx
        .signature_facts
        .symbol_types
        .get(&variant.symbol)?
        .clone()
    else {
        return None;
    };
    Some(super::expr::instantiate_callable_signature(ctx, signature))
}

pub(super) fn invalid(
    ctx: &mut BodyCollectContext<'_, '_>,
    span: Span,
    message: impl Into<String>,
) {
    ctx.validate(crate::ValidationRequest::Diagnostic {
        code: TypeDiagnosticCode::TypeMismatch,
        span,
        message: message.into(),
    });
}

pub(super) fn collect_record(
    ctx: &mut BodyCollectContext<'_, '_>,
    record: &HirRecordExpr,
    expected: Option<TypeId>,
) -> Option<TypeId> {
    let path = record.path.as_ref()?;
    let variant = declaration(ctx, path)?;
    let signature = signature(ctx, path)?;
    let Some(names) = variant.field_names else {
        invalid(
            ctx,
            record.span,
            "positional or nullary variant cannot use named-field construction",
        );
        return Some(signature.output);
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
    for field in &record.fields {
        let (name, span) = match field {
            HirFieldInit::Named { name, span, .. } | HirFieldInit::Shorthand { name, span, .. } => {
                (name, *span)
            }
        };
        if !seen.insert(name) {
            invalid(ctx, span, format!("duplicate variant field `{name}`"));
        }
        let target = names
            .iter()
            .position(|candidate| candidate == name)
            .and_then(|index| signature.params.get(index))
            .copied();
        if target.is_none() {
            invalid(ctx, span, format!("unknown variant field `{name}`"));
        }
        let actual = super::record::collect_field_value(ctx, field, target);
        if let Some(target) = target {
            ctx.emit(TypeConstraint::Assignable {
                from: actual,
                to: target,
                origin: ConstraintOrigin { span },
                reason: crate::AssignabilityReason::Argument,
            });
        }
    }
    for name in &names {
        if !seen.contains(name) {
            invalid(ctx, record.span, format!("missing variant field `{name}`"));
        }
    }
    Some(signature.output)
}

pub(super) fn collect_pattern(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &ResolvedPath,
    fields: &[etas_hir::HirRecordPatField],
    span: Span,
    ty: TypeId,
) -> bool {
    let Some(variant) = declaration(ctx, path) else {
        return false;
    };
    let Some(signature) = signature(ctx, path) else {
        return false;
    };
    let Some(names) = variant.field_names else {
        invalid(
            ctx,
            span,
            "positional or nullary variant cannot use a named-field pattern",
        );
        return true;
    };
    ctx.emit(TypeConstraint::Equal {
        lhs: signature.output,
        rhs: ty,
        origin: ConstraintOrigin { span },
    });
    let mut seen = std::collections::HashSet::new();
    for field in fields {
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
                super::pattern::collect_pattern(ctx, pat, target);
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
        if !seen.contains(name) {
            invalid(ctx, span, format!("missing variant pattern field `{name}`"));
        }
    }
    true
}
