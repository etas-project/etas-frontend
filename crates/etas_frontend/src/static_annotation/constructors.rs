use etas_code::descriptor::AnnotationConstructorKind;
use etas_hir::{ResolveResult, ResolvedPath, SymbolDef};
use etas_std::StdDecl;

pub(super) fn kind(
    checked: &crate::CheckedProgramData,
    path: &ResolvedPath,
) -> Result<AnnotationConstructorKind, String> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return Err("annotation constructor is not resolved".into());
    };
    if etas_types::facts::enums::standard_limit_constructor(
        &checked.hir,
        &checked.std_registry,
        symbol,
    )
    .is_some()
    {
        return Ok(AnnotationConstructorKind::Limit);
    }
    let symbol = checked
        .hir
        .symbols
        .get(symbol)
        .ok_or("annotation constructor symbol is missing")?;
    let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
        return Err("annotation constructor must reference a standard declaration".into());
    };
    let symbol = checked
        .std_registry
        .lookup_qualified(path)
        .ok_or("annotation constructor is absent from the checked standard registry")?;
    match &symbol.decl {
        StdDecl::Flow(flow)
            if symbol.qualified_path.as_slice()
                == ["std", "runtime", "trace", symbol.name.as_str()]
                && flow.public_effects.is_empty()
                && flow.requested_actions.is_empty() =>
        {
            Ok(AnnotationConstructorKind::Trace)
        }
        _ => Err("annotation constructor is not a checked limit or pure trace descriptor".into()),
    }
}
