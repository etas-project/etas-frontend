use etas_code::{DefinitionKey, DefinitionKind};
use etas_hir::{HirAnnotationArg, HirExpr, HirExprId, HirItemId, ResolveResult};

use crate::{
    CheckedProgramData,
    code::{descriptor::ProjectionError, identity::DefinitionIdentityIndex},
};

pub(crate) fn declared_tools(
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

impl super::DefinitionCatalog {
    pub fn tool_reference_for_symbol(
        &self,
        symbol: etas_hir::SymbolId,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<(DefinitionKey, etas_code::RevisionInfo), ProjectionError> {
        let identity = self
            .identities
            .symbol(symbol)
            .ok_or(ProjectionError::MissingDeclaration(symbol))?;
        let revision = self.tool_reference(identity, syntax)?;
        Ok((identity.clone(), revision))
    }

    /// Descriptive identity and implementation coordinates from the same
    /// checked catalog used by reflection. Neither grants tool authority.
    pub fn tool_reference(
        &self,
        identity: &DefinitionKey,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<etas_code::RevisionInfo, ProjectionError> {
        if identity.kind() != DefinitionKind::Tool {
            return Err(ProjectionError::Invalid(
                "tool reference requires a native tool declaration".into(),
            ));
        }
        let callable = etas_code::CallableId::declaration(identity.clone())?;
        if let Some(external) = self.external_callables.get(&callable) {
            let snapshot = external.snapshot.as_ref().ok_or_else(|| {
                ProjectionError::MissingIdentity("external tool implementation snapshot".into())
            })?;
            if snapshot.identity != *identity || snapshot.revision.definition != external.canonical
            {
                return Err(ProjectionError::Invalid(
                    "external tool revision disagrees with its declaration".into(),
                ));
            }
            return Ok(etas_code::RevisionInfo::new(snapshot.revision.clone()));
        }
        let header = self
            .callable(&callable)
            .ok_or_else(|| ProjectionError::MissingIdentity("tool callable header".into()))?;
        let item = self
            .item_owner(&callable)
            .ok_or_else(|| ProjectionError::MissingIdentity("tool declaration".into()))?;
        if !matches!(
            self.checked.items.get(item),
            Some(etas_hir::HirItem::Tool(_))
        ) {
            return Err(ProjectionError::Invalid(
                "tool identity resolves to a non-tool declaration".into(),
            ));
        }
        let contract = super::FrozenDefinitionContract::capture_tool(self, header)?;
        let environment = self.freeze_contract(contract, item, None)?;
        self.source_revision(&environment, syntax)
            .map(etas_code::RevisionInfo::new)
    }
}
