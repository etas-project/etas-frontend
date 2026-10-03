use std::collections::HashMap;

use crate::{CheckedSpecRef, ExternalPackageKey, pipeline::context::TypePipelineContext};

pub(crate) fn resolve_external_spec(
    ctx: &TypePipelineContext<'_>,
    package: ExternalPackageKey,
    path: &[String],
    arity: usize,
    bindings: &HashMap<(ExternalPackageKey, Vec<String>), etas_hir::SymbolId>,
) -> Result<CheckedSpecRef, String> {
    if path.first().is_some_and(|segment| segment == "std") {
        let segments = path.iter().map(String::as_str).collect::<Vec<_>>();
        let symbol = ctx
            .std_registry
            .lookup_qualified(&segments)
            .ok_or_else(|| format!("unknown standard spec `{}`", path.join(".")))?;
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
        // The metadata carrier is not the declaration owner. Std evidence uses
        // registry identity and never needs a consumer-local HIR import alias.
        return Ok(CheckedSpecRef::Std(symbol.qualified_path.clone()));
    }
    bindings
        .get(&(package, path.to_vec()))
        .copied()
        .map(CheckedSpecRef::Source)
        .ok_or_else(|| "without a checked package binding".into())
}
