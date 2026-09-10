use crate::TypeId;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnumLayoutFact {
    pub type_params: Vec<String>,
    pub variants: Vec<EnumVariantLayoutFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnumVariantLayoutFact {
    pub name: String,
    pub fields: Vec<TypeId>,
}
