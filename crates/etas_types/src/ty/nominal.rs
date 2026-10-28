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
}

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EnumTypeRef {
    pub name: String,
}
