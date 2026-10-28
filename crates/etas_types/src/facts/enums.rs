use crate::TypeId;

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

pub(crate) fn constructor_identity(
    store: &crate::TypeStore,
    symbol: &etas_hir::Symbol,
    fact: &crate::SymbolTypeFact,
) -> Option<(TypeId, usize)> {
    let index = match &symbol.def {
        etas_hir::SymbolDef::EnumVariant { variant_index, .. }
        | etas_hir::SymbolDef::ExternalEnumVariant { variant_index, .. } => *variant_index as usize,
        _ => return None,
    };
    let crate::SymbolTypeFact::Flow { signature } = fact else {
        return None;
    };
    let owner = match store.get(signature.output)? {
        crate::Type::Applied { constructor, .. } => TypeId(constructor.0),
        crate::Type::Enum(_) => signature.output,
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
