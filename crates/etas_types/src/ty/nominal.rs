use super::TypeId;

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct NamedTypeRef {
    pub name: String,
    pub source_parameter: Option<etas_hir::SymbolId>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct NominalTypeRef {
    pub name: String,
    pub params: Vec<String>,
    pub representation: Option<TypeId>,
    /// Explicit declaration permission, never inferred from an omitted argument.
    pub read_only_erased_view: bool,
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EnumTypeRef {
    pub name: String,
}
