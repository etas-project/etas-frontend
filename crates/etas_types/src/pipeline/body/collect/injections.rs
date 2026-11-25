use etas_hir::{HirAnnotationArg, HirItem, HirType, HirTypeDeclBody};

use crate::{
    AssignabilityReason, ConstraintOrigin, TypeConstraint,
    pipeline::{
        body::state::BodyPipelineState,
        context::{BodyCollectContext, TypePipelineContext},
    },
};

pub(in crate::pipeline) fn fields(
    hir: &etas_hir::HirProgram,
    item: etas_hir::HirItemId,
) -> &[etas_hir::HirFieldDecl] {
    let representation = match &hir.items[item] {
        HirItem::Type(declaration) => match declaration.body {
            HirTypeDeclBody::Representation(ty) => Some(ty),
            _ => None,
        },
        HirItem::Agent(declaration) => declaration.state,
        _ => None,
    };
    match representation.map(|ty| &hir.types[ty]) {
        Some(HirType::Record { fields, .. }) => fields,
        _ => &[],
    }
}

/// Check the declaration even when every construction supplies this field.
/// The action template gets the ordinary overload, generic and spec checks;
/// only the construction's copied expression can execute a request.
pub(super) fn collect<E>(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let fields = fields(ctx.hir, state.item).to_vec();
    for field in fields {
        for annotation in field.annotations {
            let [HirAnnotationArg::Positional { value, span }] = annotation.args.as_slice() else {
                continue;
            };
            let Some(expected) = crate::lower::type_ref::lower_type_ref(ctx, field.ty) else {
                continue;
            };
            let mut body = BodyCollectContext {
                ctx,
                state,
                execution: checkpoint,
            };
            let actual = super::expr::collect_expr(&mut body, *value, Some(expected))?;
            body.emit(TypeConstraint::Assignable {
                from: actual,
                to: expected,
                origin: ConstraintOrigin { span: *span },
                reason: AssignabilityReason::Constructor,
            });
        }
    }
    Ok(())
}
