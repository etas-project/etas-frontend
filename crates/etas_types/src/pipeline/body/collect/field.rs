use crate::{Type, TypeId, pipeline::context::BodyCollectContext};

// Lower the declared representation before the read-only relation solver runs.
// This supplies a field type hint, not a nominal-to-record assignability rule.
pub(super) fn declared_field_hint(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: TypeId,
    field: &str,
    span: etas_core::Span,
) -> Option<TypeId> {
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
