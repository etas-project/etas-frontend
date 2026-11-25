use std::collections::HashSet;

use etas_hir::{HirExpr, HirProgram};
use etas_package_metadata::MethodReceiver;

use crate::{ProjectContext, ProjectEnvironmentInput, ProjectExternalTypeInput};

pub(super) fn elaborate(context: &ProjectContext, hir: &mut HirProgram) -> Result<(), String> {
    let Some(environment) = context.validated_external_environment.as_ref() else {
        return Ok(());
    };
    let environment = environment.environment();
    if environment.external_public_metadata.is_empty() {
        return Ok(());
    }
    let canonical = super::constructor_injection::canonical_imports(context, hir);
    let imported = hir
        .exprs
        .iter()
        .flat_map(|(_, value)| match value {
            HirExpr::Pipeline { stages, .. } | HirExpr::StageCompose { stages, .. } => {
                stages.iter().map(|stage| stage.expr).collect::<Vec<_>>()
            }
            _ => Vec::new(),
        })
        .filter_map(|expr| {
            let HirExpr::Path(path) = &hir.exprs[expr] else {
                return None;
            };
            let path = super::constructor_injection::external_path(hir, path, &canonical)?;
            let owner = empty_type(environment, &path)?;
            environment
                .external_public_metadata
                .iter()
                .flat_map(|metadata| &metadata.methods)
                .any(|method| {
                    !method.internal
                        && method.identity.name() == "run"
                        && matches!(
                            method.receiver,
                            MethodReceiver::Value { .. } | MethodReceiver::Updating { .. }
                        )
                        && method.self_type.path == owner
                })
                .then_some(expr)
        })
        .collect();
    hir.elaborate_type_stages(&imported)
        .map_err(|reason| format!("cannot elaborate type stages: {reason}"))
}

fn empty_type(environment: &ProjectEnvironmentInput, path: &[String]) -> Option<Vec<String>> {
    let mut path = path.to_vec();
    let mut seen = HashSet::new();
    while seen.insert(path.clone()) {
        if let Some(contract) = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.constructor_contracts)
            .find(|contract| contract.path == path)
        {
            return contract.fields.is_empty().then_some(path);
        }
        if let Some(export) = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.re_exports)
            .find(|export| export.exported == path)
        {
            path = export.from.clone();
            continue;
        }
        let ty = environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| &metadata.types)
            .find(|signature| signature.path == path)?
            .ty
            .as_ref()?;
        match ty {
            ProjectExternalTypeInput::Alias { target, .. } => {
                path = type_path(target)?.to_vec();
            }
            ProjectExternalTypeInput::Nominal {
                representation: Some(representation),
                ..
            } if matches!(representation.as_ref(), ProjectExternalTypeInput::Record { fields } if fields.is_empty()) =>
            {
                return Some(path);
            }
            ProjectExternalTypeInput::Record { fields } if fields.is_empty() => return Some(path),
            _ => return None,
        }
    }
    None
}

fn type_path(ty: &ProjectExternalTypeInput) -> Option<&[String]> {
    match ty {
        ProjectExternalTypeInput::Named(path)
        | ProjectExternalTypeInput::Applied { path, .. }
        | ProjectExternalTypeInput::Nominal { path, .. } => Some(path),
        ProjectExternalTypeInput::Alias { target, .. } => type_path(target),
        _ => None,
    }
}
