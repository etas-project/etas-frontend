use crate::{EffectArgRef, TypeId};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct CheckedHandlerActionFact {
    pub action_symbol: etas_hir::SymbolId,
    pub selectors: Vec<EffectArgRef>,
    pub params: Vec<TypeId>,
    pub output: TypeId,
}

/// A contextual handler type is not a proof until all reachable request
/// instances have been checked by the project-level invocation analysis.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct DeferredHandlerInstantiation {
    pub body: etas_hir::HirExprId,
    pub parameters: Vec<HandlerTypeBinding>,
    pub span: etas_core::Span,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct HandlerTypeBinding {
    pub parameter: String,
    pub candidate: TypeId,
    pub rigid: TypeId,
    pub parametric: bool,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EffectActionSignature {
    #[serde(default)]
    pub generic_params: Vec<crate::CallableGenericParam>,
    pub params: Vec<TypeId>,
    pub output: TypeId,
    pub effect_args: Vec<EffectActionArgKind>,
    #[serde(default)]
    pub selector_param_names: Vec<String>,
    #[serde(default)]
    pub selector_defaults: Vec<Option<EffectArgRef>>,
    pub returns_never: bool,
    #[serde(default)]
    pub default_handler_effects: Option<crate::EffectRowRef>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
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
