use etas_hir::{HirPat, ResolveResult, ResolvedPath};

use crate::{
    AssignabilityReason, CallableSignature, ConstraintOrigin, FieldType, FlowType, RecordType,
    SymbolTypeFact, Type, TypeConstraint, TypeId,
    pipeline::{body::collect::literal::collect_literal, context::BodyCollectContext},
};

pub fn collect_pattern<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    pat: etas_hir::HirPatId,
    ty: TypeId,
) -> Result<(), E> {
    ctx.checkpoint()?;
    ctx.record_pat_type(pat, ty);
    match ctx.ctx.hir.pats[pat].clone() {
        HirPat::Binding { symbol, .. } => {
            ctx.record_symbol_type(symbol, SymbolTypeFact::Local { ty, mutable: false });
        }
        HirPat::Wildcard { .. } => {}
        HirPat::Literal(literal) => {
            let lit_ty = collect_literal(ctx, None, &literal, Some(ty));
            ctx.emit(TypeConstraint::Assignable {
                from: lit_ty,
                to: ty,
                origin: ConstraintOrigin {
                    span: literal.span(),
                },
                reason: AssignabilityReason::Pattern,
            });
        }
        HirPat::Tuple { elems, span } => {
            let expected_elems = match ctx.ctx.interner.store().get(ty) {
                Some(Type::Tuple(elems)) => Some(elems.clone()),
                _ => None,
            };
            let elem_tys = elems
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    expected_elems
                        .as_ref()
                        .and_then(|elems| elems.get(index).copied())
                        .map(|ty| normalize_pattern_binding_type(ctx, ty))
                        .unwrap_or_else(|| ctx.fresh_type_var())
                })
                .collect::<Vec<_>>();
            let tuple_ty = ctx.ctx.interner.intern(Type::Tuple(elem_tys.clone()));
            ctx.emit(TypeConstraint::Assignable {
                from: ty,
                to: tuple_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Pattern,
            });
            for (elem, elem_ty) in elems.into_iter().zip(elem_tys.into_iter()) {
                collect_pattern(ctx, elem, elem_ty)?;
            }
        }
        HirPat::Record { path, fields, span } => {
            if let Some(path) = &path {
                if super::enum_variant::collect_pattern(ctx, path, &fields, span, ty)? {
                    return Ok(());
                }
            }
            let expected_fields = record_fields(ctx, ty, span);
            let field_tys = fields
                .iter()
                .map(|field| FieldType {
                    name: field.name.clone(),
                    ty: expected_fields
                        .as_ref()
                        .and_then(|record| {
                            record
                                .fields
                                .iter()
                                .find(|candidate| candidate.name == field.name)
                                .map(|candidate| candidate.ty)
                        })
                        .unwrap_or_else(|| ctx.fresh_type_var()),
                })
                .collect::<Vec<_>>();
            let record_ty = ctx.ctx.interner.intern(Type::Record(RecordType {
                fields: field_tys.clone(),
            }));
            ctx.emit(TypeConstraint::Assignable {
                from: ty,
                to: record_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Pattern,
            });
            if let Some(expected_fields) = expected_fields {
                for field in &fields {
                    ctx.checkpoint()?;
                    if !expected_fields
                        .fields
                        .iter()
                        .any(|candidate| candidate.name == field.name)
                    {
                        ctx.validate(crate::ValidationRequest::Diagnostic {
                            code: etas_core::TypeDiagnosticCode::MissingField,
                            span,
                            message: format!(
                                "record pattern references missing field `{}`",
                                field.name
                            ),
                        });
                    }
                }
            }
            for field in fields {
                if let Some(pat) = field.pat
                    && let Some(field_ty) = field_tys
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                        .map(|candidate| candidate.ty)
                {
                    collect_pattern(ctx, pat, field_ty)?;
                }
            }
        }
        HirPat::Variant { path, args, span } => {
            collect_variant_pattern(ctx, &path, args, span, ty)?;
        }
        HirPat::Error { span } => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "error pattern cannot be typed".to_owned(),
            });
        }
    }
    Ok(())
}

fn collect_variant_pattern<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
    args: Vec<etas_hir::HirPatId>,
    span: etas_core::Span,
    ty: TypeId,
) -> Result<(), E> {
    ctx.checkpoint()?;
    if let Some(variant) = super::enum_variant::declaration(ctx, path) {
        if variant.field_names.is_some() {
            super::enum_variant::invalid(
                ctx,
                span,
                "named-field variant requires a named-field pattern",
            );
            return Ok(());
        }
        return collect_constructor_variant(ctx, path, args, span, ty);
    }
    match variant_leaf_name(path).as_deref() {
        Some("Ok") => collect_result_variant(ctx, "Ok", args, span, ty, ResultVariant::Ok),
        Some("Err") => collect_result_variant(ctx, "Err", args, span, ty, ResultVariant::Err),
        Some("Some") => collect_option_variant(ctx, "Some", args, span, ty, OptionVariant::Some),
        Some("None") => collect_option_variant(ctx, "None", args, span, ty, OptionVariant::None),
        _ => collect_constructor_variant(ctx, path, args, span, ty),
    }
}

#[derive(Clone, Copy)]
enum ResultVariant {
    Ok,
    Err,
}

fn collect_result_variant<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    args: Vec<etas_hir::HirPatId>,
    span: etas_core::Span,
    ty: TypeId,
    variant: ResultVariant,
) -> Result<(), E> {
    ctx.checkpoint()?;
    let (ok, err) = match ctx.ctx.interner.store().get(ty).cloned() {
        Some(Type::Result { ok, err }) => (ok, err),
        _ => (ctx.fresh_type_var(), ctx.fresh_type_var()),
    };
    let result_ty = ctx.ctx.interner.intern(Type::Result { ok, err });
    ctx.emit(TypeConstraint::Equal {
        lhs: ty,
        rhs: result_ty,
        origin: ConstraintOrigin { span },
    });
    let payload = match variant {
        ResultVariant::Ok => ok,
        ResultVariant::Err => err,
    };
    collect_variant_args(ctx, name, args, &[payload], span)
}

#[derive(Clone, Copy)]
enum OptionVariant {
    Some,
    None,
}

fn collect_option_variant<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    args: Vec<etas_hir::HirPatId>,
    span: etas_core::Span,
    ty: TypeId,
    variant: OptionVariant,
) -> Result<(), E> {
    ctx.checkpoint()?;
    let inner = match ctx.ctx.interner.store().get(ty).cloned() {
        Some(Type::Option(inner)) => inner,
        _ => ctx.fresh_type_var(),
    };
    let option_ty = ctx.ctx.interner.intern(Type::Option(inner));
    ctx.emit(TypeConstraint::Equal {
        lhs: ty,
        rhs: option_ty,
        origin: ConstraintOrigin { span },
    });
    match variant {
        OptionVariant::Some => collect_variant_args(ctx, name, args, &[inner], span),
        OptionVariant::None => collect_variant_args(ctx, name, args, &[], span),
    }
}

fn collect_constructor_variant<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
    args: Vec<etas_hir::HirPatId>,
    span: etas_core::Span,
    ty: TypeId,
) -> Result<(), E> {
    ctx.checkpoint()?;
    let Some(signature) = constructor_signature(ctx, path) else {
        for arg in args {
            let arg_ty = ctx.fresh_type_var();
            collect_pattern(ctx, arg, arg_ty)?;
        }
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::UnknownType,
            span,
            message: format!(
                "variant pattern constructor `{}` is not resolved to a constructor signature",
                variant_leaf_name(path).unwrap_or_else(|| "<unknown>".to_owned())
            ),
        });
        return Ok(());
    };

    let constructor_name = variant_leaf_name(path).unwrap_or_else(|| "<constructor>".to_owned());
    if args.len() != signature.params.len() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span,
            message: format!(
                "variant pattern `{constructor_name}` expects {} argument(s), got {}",
                signature.params.len(),
                args.len()
            ),
        });
    }

    let Some(signature) = super::expr::instantiate_callable_signature(ctx, signature) else {
        return Ok(());
    };
    ctx.emit(TypeConstraint::Equal {
        lhs: signature.output,
        rhs: ty,
        origin: ConstraintOrigin { span },
    });
    let arg_tys = args
        .into_iter()
        .enumerate()
        .map(|(index, arg)| {
            let arg_ty = signature
                .params
                .get(index)
                .copied()
                .unwrap_or_else(|| ctx.fresh_type_var());
            collect_pattern(ctx, arg, arg_ty)?;
            Ok(arg_ty)
        })
        .collect::<Result<Vec<_>, E>>()?;
    let callee = ctx.ctx.interner.intern(Type::Function(FlowType {
        input: signature.params,
        output: signature.output,
        effects: None,
    }));
    ctx.emit(TypeConstraint::Callable {
        call: None,
        callee,
        generic_params: signature.generic_params,
        generic_args: Vec::new(),
        arg_exprs: Vec::new(),
        args: arg_tys,
        output: ty,
        origin: ConstraintOrigin { span },
    });
    Ok(())
}

fn constructor_signature<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
) -> Option<CallableSignature> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    match ctx
        .state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))?
    {
        SymbolTypeFact::Flow { signature } | SymbolTypeFact::Tool { signature } => {
            Some(signature.clone())
        }
        SymbolTypeFact::Value { ty } => Some(CallableSignature {
            generic_params: Vec::new(),
            params: Vec::new(),
            output: *ty,
            effects: None,
            requested_actions: None,
        }),
        _ => None,
    }
}

fn collect_variant_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    args: Vec<etas_hir::HirPatId>,
    expected: &[TypeId],
    span: etas_core::Span,
) -> Result<(), E> {
    ctx.checkpoint()?;
    if args.len() != expected.len() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span,
            message: format!(
                "variant pattern `{}` expects {} argument(s), got {}",
                name,
                expected.len(),
                args.len()
            ),
        });
    }
    for (arg, arg_ty) in args.iter().copied().zip(expected.iter().copied()) {
        let arg_ty = normalize_pattern_binding_type(ctx, arg_ty);
        collect_pattern(ctx, arg, arg_ty)?;
    }
    if args.len() > expected.len() {
        for arg in args.into_iter().skip(expected.len()) {
            let arg_ty = ctx.fresh_type_var();
            collect_pattern(ctx, arg, arg_ty)?;
        }
    }
    Ok(())
}

fn variant_leaf_name(path: &ResolvedPath) -> Option<String> {
    path.segments.last().map(|segment| segment.name.clone())
}

fn normalize_pattern_binding_type<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: TypeId,
) -> TypeId {
    match ctx.ctx.interner.store().get(ty) {
        Some(Type::IntegerLiteral { .. }) => ctx.primitive(crate::PrimitiveType::I32),
        _ => ty,
    }
}

fn record_fields<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: TypeId,
    span: etas_core::Span,
) -> Option<RecordType> {
    let ty = match crate::applied_representation(&mut ctx.ctx.interner, ty) {
        Ok(Some(representation)) => representation,
        Ok(None) => ty,
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("pattern representation substitution failed: {error}"),
            });
            return None;
        }
    };
    match ctx.ctx.interner.store().get(ty)? {
        Type::Record(record) => Some(record.clone()),
        _ => None,
    }
}
