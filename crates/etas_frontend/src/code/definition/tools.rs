use etas_code::{DefinitionKey, DefinitionKind};
use etas_hir::{HirAnnotationArg, HirExpr, HirExprId, HirItemId, ResolveResult};

use crate::{
    CheckedProgramData,
    code::{descriptor::ProjectionError, identity::DefinitionIdentityIndex},
};

pub(super) fn declared_tools(
    checked: &CheckedProgramData,
    identities: &DefinitionIdentityIndex,
    item: HirItemId,
) -> Result<Vec<DefinitionKey>, ProjectionError> {
    let mut tools = Vec::new();
    for annotation in checked
        .hir
        .item_annotations
        .get(&item)
        .into_iter()
        .flatten()
    {
        if annotation.path.segments.len() != 1 || annotation.path.segments[0].name != "tools" {
            continue;
        }
        // The existing annotation application replaces the model tool surface
        // for each @tools annotation; reflection must describe that same surface.
        tools.clear();
        for argument in &annotation.args {
            let value = match argument {
                HirAnnotationArg::Positional { value, .. } => *value,
                HirAnnotationArg::Named { name, .. } if name == "choice" || name == "mode" => {
                    continue;
                }
                HirAnnotationArg::Named { name, value, .. }
                    if name == "tools" || name == "items" =>
                {
                    *value
                }
                HirAnnotationArg::Named { .. } => {
                    return Err(ProjectionError::Invalid(
                        "checked tool annotation contains an unsupported argument".into(),
                    ));
                }
            };
            match checked.hir.exprs.get(value) {
                Some(HirExpr::Array { elems, .. }) | Some(HirExpr::List { elems, .. }) => {
                    for entry in elems {
                        tools.push(tool(checked, identities, *entry)?);
                    }
                }
                Some(HirExpr::EmptySequence { .. }) => {}
                _ => tools.push(tool(checked, identities, value)?),
            }
        }
    }
    Ok(tools)
}

fn tool(
    checked: &CheckedProgramData,
    identities: &DefinitionIdentityIndex,
    expression: HirExprId,
) -> Result<DefinitionKey, ProjectionError> {
    let Some(HirExpr::Path(path)) = checked.hir.exprs.get(expression) else {
        return Err(ProjectionError::Invalid(
            "checked tool annotation entry is not a resolved declaration path".into(),
        ));
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return Err(ProjectionError::Invalid(
            "checked tool annotation has no resolved declaration".into(),
        ));
    };
    let identity = identities
        .symbol(symbol)
        .ok_or(ProjectionError::MissingDeclaration(symbol))?;
    if identity.kind() != DefinitionKind::Tool {
        return Err(ProjectionError::Invalid(
            "checked tool annotation points to a non-tool declaration".into(),
        ));
    }
    Ok(identity.clone())
}
