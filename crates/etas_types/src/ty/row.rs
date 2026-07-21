#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct EffectRowRef {
    pub effects: Vec<EffectRef>,
    pub tail: Option<String>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct EffectRef {
    pub name: String,
    pub args: Vec<EffectArgRef>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum EffectArgRef {
    Type(super::TypeId),
    Path(Vec<String>),
    String(String),
    Int(String),
    Wildcard,
}
