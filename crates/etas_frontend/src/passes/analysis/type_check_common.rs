use etas_utils::{PassContext, PassFailure};

use crate::{BODY_UNIT_KIND, ProjectContext, UnitId, UnitTarget};

pub(crate) fn hir_item_for_body_unit(
    context: &ProjectContext,
    pass_context: &PassContext<ProjectContext>,
) -> Result<etas_hir::HirItemId, PassFailure> {
    let unit = body_unit_id(pass_context)?;
    let tree = context.units.as_ref().expect("unit tree should exist");
    let node = tree.nodes.get(unit).expect("body unit id should resolve");
    let UnitTarget::AstBody(body) = &node.target else {
        return Err(PassFailure::new(format!(
            "body unit {:?} does not target an AST body",
            unit
        )));
    };
    context
        .hir_item_bindings
        .as_ref()
        .expect("HIR item bindings should exist")
        .ast_to_hir
        .get(&body.item)
        .copied()
        .ok_or_else(|| PassFailure::new("body AST item is missing its explicit HIR item binding"))
}

pub(crate) fn body_unit_id(
    pass_context: &PassContext<ProjectContext>,
) -> Result<UnitId, PassFailure> {
    let Some(unit) = pass_context.current_unit else {
        return Err(PassFailure::new(
            "body-scoped pass was invoked without a current unit",
        ));
    };
    if unit.kind != BODY_UNIT_KIND {
        return Err(PassFailure::new(format!(
            "body-scoped pass received mismatched unit kind {:?}",
            unit.kind
        )));
    }
    Ok(UnitId(unit.id as u32))
}
