use crate::{CheckedSpecRef, ExternalPackageKey, pipeline::context::TypePipelineContext};

pub(crate) fn resolve_external_spec(
    ctx: &TypePipelineContext<'_>,
    package: ExternalPackageKey,
    path: &[String],
    arity: usize,
) -> Result<CheckedSpecRef, String> {
    resolve_external_spec_path(ctx, path, arity)
        .map_err(|reason| format!("package {}: {reason}", package.0))
}

pub(super) fn resolve_external_spec_path(
    ctx: &TypePipelineContext<'_>,
    path: &[String],
    arity: usize,
) -> Result<CheckedSpecRef, String> {
    if let Some(symbol) = ctx.std_registry.lookup_qualified(path)
        && ctx.std_registry.declaration_key(symbol.id).is_none()
    {
        let etas_std::StdDecl::Type(decl) = &symbol.decl else {
            return Err(format!("`{}` is not a standard type spec", path.join(".")));
        };
        if decl.kind != etas_std::TypeDeclKind::Spec {
            return Err(format!("`{}` is not a standard type spec", path.join(".")));
        }
        if decl.params.len() != arity {
            return Err(format!(
                "standard spec `{}` expects {} arguments, got {arity}",
                path.join("."),
                decl.params.len()
            ));
        }
        // Native contracts use registry identity. Source-owned std specs use
        // the checked SDK declaration and method facts below.
        return Ok(CheckedSpecRef::Std(symbol.qualified_path.clone()));
    }
    let mut declarations = ctx
        .signature_facts
        .external_specs
        .signatures
        .iter()
        .filter(|(identity, _)| identity.path == path);
    let (identity, signature) = declarations
        .next()
        .ok_or("external metadata references a spec without a checked declaration")?;
    if declarations.next().is_some() {
        return Err("external spec path has more than one package owner".into());
    }
    if signature.kind != crate::SpecKind::TypeSpec || signature.param_names.len() != arity {
        return Err(format!(
            "external spec `{}` has incompatible kind or generic arity",
            path.join(".")
        ));
    }
    Ok(CheckedSpecRef::External(identity.clone()))
}
