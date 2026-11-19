use crate::TypeId;

/// Resolve a trusted std source enum constructor to the runtime budget rule.
/// Source owns the enum and variant symbols; the kernel owns budget semantics.
pub fn standard_limit_constructor(
    hir: &etas_hir::HirProgram,
    registry: &etas_std::StdRegistry,
    symbol: etas_hir::SymbolId,
) -> Option<etas_std::StdLimitKind> {
    let symbol = hir.canonical_source_symbol(symbol)?;
    let data = hir.symbols.get(symbol)?;
    if let etas_hir::SymbolDef::ExternalEnumVariant {
        enum_path,
        variant_index,
        ..
    } = &data.def
    {
        let owner = registry.lookup_qualified(enum_path)?;
        let constructor = registry
            .enum_constructors(owner.id)
            .nth(*variant_index as usize)?;
        return source_limit_constructor(registry, constructor);
    }
    let path = match &data.def {
        etas_hir::SymbolDef::ImportAlias { path, .. } => path.clone(),
        etas_hir::SymbolDef::EnumVariant {
            enum_item,
            variant_index,
        } if hir.source_standard_types => {
            let etas_hir::HirItem::Enum(owner) = hir.items.get(*enum_item)? else {
                return None;
            };
            if owner.variants.get(*variant_index as usize)?.symbol != symbol {
                return None;
            }
            let owner = hir.symbols.get(owner.symbol)?;
            let module = hir
                .modules_arena
                .get(owner.defining_module)?
                .name
                .as_ref()?;
            if owner.name != "Limit"
                || !module
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(["std", "runtime", "limits"])
            {
                return None;
            }
            vec![
                "std".into(),
                "runtime".into(),
                "limits".into(),
                data.name.clone(),
            ]
        }
        _ => return None,
    };
    let declaration = registry.lookup_qualified(&path)?;
    match &declaration.decl {
        etas_std::StdDecl::Requirement(requirement) => match requirement.semantics {
            etas_std::RequirementSemantics::Limit(kind) => Some(kind),
            _ => None,
        },
        _ => source_limit_constructor(registry, declaration),
    }
}

fn source_limit_constructor(
    registry: &etas_std::StdRegistry,
    constructor: &etas_std::StdSymbol,
) -> Option<etas_std::StdLimitKind> {
    let owner = registry.symbol(constructor.enum_owner?)?;
    let definition = registry.declaration_key(owner.id)?;
    if definition.kind() != etas_code::DefinitionKind::Type
        || !definition
            .module()
            .iter()
            .chain(definition.declaration())
            .map(String::as_str)
            .eq(["std", "runtime", "limits", "Limit"])
    {
        return None;
    }
    // The producer checks these payloads against the execution ABI. The
    // source enum keeps its declaration, layout and ordinary constructor.
    use etas_std::StdLimitKind;
    match constructor.name.as_str() {
        "Iterations" => Some(StdLimitKind::Iterations),
        "Tokens" => Some(StdLimitKind::Tokens),
        "ContextTokens" => Some(StdLimitKind::ContextTokens),
        "Cost" => Some(StdLimitKind::Cost),
        "WallTime" => Some(StdLimitKind::WallTime),
        "Attempts" => Some(StdLimitKind::Attempts),
        "Concurrency" => Some(StdLimitKind::Concurrency),
        _ => None,
    }
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EnumLayoutFact {
    pub type_params: Vec<String>,
    pub variants: Vec<EnumVariantLayoutFact>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EnumVariantLayoutFact {
    pub name: String,
    pub fields: Vec<TypeId>,
    pub field_names: Option<Vec<String>>,
}

pub(crate) fn constructor_name(symbol: &etas_hir::Symbol) -> &str {
    if let etas_hir::SymbolDef::ImportAlias {
        path,
        origin: etas_hir::ImportAliasOrigin::StdEnumVariant { .. },
    } = &symbol.def
    {
        return path.last().map_or(&symbol.name, String::as_str);
    }
    &symbol.name
}

pub(crate) fn constructor_identity(
    store: &crate::TypeStore,
    symbol: &etas_hir::Symbol,
    fact: &crate::SymbolTypeFact,
    known: &crate::KnownStdTypes,
) -> Option<(TypeId, usize)> {
    let index = match &symbol.def {
        etas_hir::SymbolDef::EnumVariant { variant_index, .. }
        | etas_hir::SymbolDef::ExternalEnumVariant { variant_index, .. } => *variant_index as usize,
        etas_hir::SymbolDef::ImportAlias {
            origin: etas_hir::ImportAliasOrigin::StdEnumVariant { variant_index },
            ..
        } => *variant_index as usize,
        _ => return None,
    };
    let crate::SymbolTypeFact::Flow { signature } = fact else {
        return None;
    };
    let owner = match store.get(signature.output)? {
        crate::Type::Applied { constructor, .. } => TypeId(constructor.0),
        crate::Type::Enum(_) => signature.output,
        crate::Type::Result { .. } => known.result_enum?,
        crate::Type::Option(_) => known.option_enum?,
        _ => return None,
    };
    let crate::Type::Enum(declaration) = store.get(owner)? else {
        return None;
    };
    if let etas_hir::SymbolDef::ExternalEnumVariant { enum_path, .. } = &symbol.def
        && !declaration
            .name
            .split('.')
            .eq(enum_path.iter().map(String::as_str))
    {
        return None;
    }
    Some((owner, index))
}
