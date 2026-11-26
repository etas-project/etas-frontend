use etas_core::Span;
use etas_hir::{HirExprId, HirRecordExpr};

use crate::pipeline::body::state::{
    BodyPipelineState, DeferredCollectionLiteral, EmptyCollectionKind,
};
use crate::pipeline::context::TypePipelineContext;
use crate::{
    Type, TypeId, ValidationRequest, method::MethodSelection, pipeline::context::BodyCollectContext,
};

pub(super) fn empty_sequence<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    expected: Option<TypeId>,
    span: Span,
) -> Result<TypeId, E> {
    if let Some(ty) = expected {
        if matches!(ctx.ctx.interner.store().get(ty), Some(Type::Array(_))) {
            return Ok(ty);
        }
        if matches!(
            ctx.ctx.interner.store().get(ty),
            Some(Type::Nominal(_) | Type::Applied { .. })
        ) && super::expr::sequence_element(ctx, ty, span).is_some()
        {
            return Ok(ty);
        }
        if !matches!(ctx.ctx.interner.store().get(ty), Some(Type::Var(_))) {
            return Ok(invalid_context(
                ctx,
                span,
                "empty sequence needs a source Array<T> or native sequence type",
            ));
        }
    }
    let ty = expected.unwrap_or_else(|| ctx.fresh_type_var());
    ctx.state
        .deferred_collection_literals
        .push(DeferredCollectionLiteral {
            expr,
            ty,
            span,
            kind: EmptyCollectionKind::Sequence,
        });
    Ok(ty)
}

pub(super) fn empty_record_or_map<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    expected: Option<TypeId>,
    span: Span,
) -> Result<TypeId, E> {
    if let Some(ty) = expected
        && !matches!(ctx.ctx.interner.store().get(ty), Some(Type::Var(_)))
    {
        if let Some(owner) = selected_owner(ctx, ty, "Map") {
            return constructor(ctx, expr, owner, ty);
        }
        // Empty record construction follows the same privacy, nominal identity
        // and field checks as a record with explicitly written fields.
        return super::record::collect_record(
            ctx,
            &HirRecordExpr {
                base: None,
                injections: None,
                path: None,
                generic_args: Vec::new(),
                fields: Vec::new(),
                span,
            },
            Some(ty),
        );
    }
    let ty = expected.unwrap_or_else(|| ctx.fresh_type_var());
    ctx.state
        .deferred_collection_literals
        .push(DeferredCollectionLiteral {
            expr,
            ty,
            span,
            kind: EmptyCollectionKind::RecordOrMap,
        });
    Ok(ty)
}

/// An argument may receive its collection type from the selected generic
/// method or from later constraints. Resolve only those empty literals after
/// that inference, then let the ordinary solver check their constructor calls.
pub(in crate::pipeline) fn finish_deferred<E>(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<bool, E> {
    let deferred = std::mem::take(&mut state.deferred_collection_literals);
    let before = state.constraints.len();
    for literal in deferred {
        checkpoint()?;
        let resolved = crate::substitute_type_params(
            &mut ctx.interner,
            literal.ty,
            &Default::default(),
            &state.solver_report.substitutions,
        );
        let mut collect = BodyCollectContext {
            ctx: &mut *ctx,
            state: &mut *state,
            execution: checkpoint,
        };
        let ty = match resolved {
            Ok(ty) if !matches!(collect.ctx.interner.store().get(ty), Some(Type::Var(_))) => ty,
            result => {
                collect.validate(ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: literal.span,
                    message: format!(
                        "empty collection needs an inferred collection type: {result:?}"
                    ),
                });
                continue;
            }
        };
        let actual = match literal.kind {
            EmptyCollectionKind::Sequence => {
                empty_sequence(&mut collect, literal.expr, Some(ty), literal.span)?
            }
            EmptyCollectionKind::RecordOrMap => {
                empty_record_or_map(&mut collect, literal.expr, Some(ty), literal.span)?
            }
        };
        collect.record_expr_type(literal.expr, actual);
        collect.emit(crate::TypeConstraint::Assignable {
            from: actual,
            to: literal.ty,
            origin: crate::ConstraintOrigin { span: literal.span },
            reason: crate::AssignabilityReason::Annotation,
        });
    }
    Ok(state.constraints.len() != before)
}

fn selected_owner<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: TypeId,
    name: &str,
) -> Option<TypeId> {
    let owner = match ctx.ctx.interner.store().get(ty)? {
        Type::Applied { constructor, .. } => TypeId(constructor.0),
        Type::Nominal(_) => ty,
        _ => return None,
    };
    let selected =
        crate::lower::type_ref::selected_source_type(ctx.ctx, &["std", "collections", name])?;
    (owner == selected).then_some(owner)
}

fn constructor<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    owner: TypeId,
    expected: TypeId,
) -> Result<TypeId, E> {
    super::method_call::collect_selected(
        ctx,
        super::method_call::SelectedMethodCall {
            expr,
            // An associated call has no evaluated receiver. The frontend reifies
            // its type path after solving, before effects or execution see the HIR.
            receiver: expr,
            method: "new",
            generic_args: &[],
            args: &[],
            selection: MethodSelection::Associated,
            expected: Some(expected),
            binding: None,
            associated_owner: Some(owner),
        },
    )
}

fn invalid_context<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    span: Span,
    message: &str,
) -> TypeId {
    ctx.validate(ValidationRequest::Diagnostic {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span,
        message: message.into(),
    });
    ctx.primitive(crate::PrimitiveType::Never)
}
