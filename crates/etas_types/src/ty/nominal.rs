use super::TypeId;

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NamedTypeRef {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NominalTypeRef {
    pub name: String,
    pub params: Vec<String>,
    pub representation: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct EnumTypeRef {
    pub name: String,
}
