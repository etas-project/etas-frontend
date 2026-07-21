use crate::{EffectArgRef, TypeId};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectActionSignature {
    pub params: Vec<TypeId>,
    pub output: TypeId,
    pub effect_args: Vec<EffectActionArgKind>,
    #[serde(default)]
    pub selector_param_names: Vec<String>,
    #[serde(default)]
    pub selector_defaults: Vec<Option<EffectArgRef>>,
    pub returns_never: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectActionArgKind {
    Type,
    MemoryPlace,
    StaticResourcePath { ty: String },
    StringPattern,
}

impl From<&EffectArgRef> for EffectActionArgKind {
    fn from(value: &EffectArgRef) -> Self {
        match value {
            EffectArgRef::Type(_) => Self::Type,
            EffectArgRef::Path(_) => Self::StaticResourcePath { ty: String::new() },
            EffectArgRef::String(_) | EffectArgRef::Int(_) | EffectArgRef::Wildcard => {
                Self::StringPattern
            }
        }
    }
}
