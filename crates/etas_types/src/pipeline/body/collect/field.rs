use crate::{Type, TypeId, pipeline::context::BodyCollectContext};

pub(super) fn agent_field_base<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    base: TypeId,
    receiver: Option<etas_hir::SymbolId>,
    field: &str,
    span: etas_core::Span,
) -> TypeId {
    let Some(receiver) = receiver else {
        return base;
    };
    let owned = match ctx.ctx.hir.items.get(ctx.state.item) {
        Some(etas_hir::HirItem::Flow(flow))
            if flow.receiver_mode != etas_hir::HirReceiverMode::None
                && flow.params.first() == Some(&receiver) =>
        {
            let component = ctx.ctx.agent_signature(base).map(|agent| agent.component);
            component.is_some()
                && ctx
                    .ctx
                    .signature_facts
                    .type_impl_methods
                    .values()
                    .chain(
                        ctx.ctx
                            .signature_base
                            .into_iter()
                            .flat_map(|base| base.facts.type_impl_methods.values()),
                    )
                    .any(|method| {
                        method.body == ctx.state.item
                            && ctx
                                .ctx
                                .agent_signature(method.self_type)
                                .map(|agent| agent.component)
                                == component
                    })
        }
        Some(etas_hir::HirItem::Tool(_)) => {
            ctx.ctx.hir.member_tool_receiver(ctx.state.item) == Some(receiver)
        }
        _ => false,
    };
    if !owned {
        return base;
    }
    match ctx.ctx.agent_state_type(base) {
        Ok(Some(state))
            if ctx.ctx.hir.member_tool_receiver(ctx.state.item) == Some(receiver)
                || matches!(ctx.ctx.interner.store().get(state), Some(Type::Record(record)) if record.fields.iter().any(|member| member.name == field)) =>
        {
            state
        }
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: error.to_string(),
            });
            base
        }
        _ => base,
    }
}

// Lower the declared representation before the read-only relation solver runs.
// This supplies a field type hint, not a nominal-to-record assignability rule.
pub(super) fn declared_field_hint<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    base: TypeId,
    field: &str,
    span: etas_core::Span,
) -> Option<TypeId> {
    if let Some(projection) = crate::value_representation::checked_value_projection_with(
        ctx.ctx.interner.store(),
        base,
        field,
        || ctx.ctx.spec_facts(),
    ) {
        return projection.output_type(ctx.ctx.interner.store(), base);
    }
    let store = ctx.ctx.interner.store();
    if let Some(schema) = crate::value_representation::memory_region_schema(store, base)
        && let Some(Type::Record(record)) = store.get(schema)
        && let Some(found) = record.fields.iter().find(|entry| entry.name == field)
    {
        return Some(found.ty);
    }
    let representation = match crate::applied_representation(&mut ctx.ctx.interner, base) {
        Ok(Some(ty)) => ty,
        Ok(None) => base,
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("field representation substitution failed: {error}"),
            });
            return None;
        }
    };
    let Type::Record(record) = ctx.ctx.interner.store().get(representation)? else {
        return None;
    };
    record
        .fields
        .iter()
        .find(|entry| entry.name == field)
        .map(|entry| entry.ty)
}
