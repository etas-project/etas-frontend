#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct EffectRowRef {
    pub effects: Vec<EffectRef>,
    pub tail: Option<String>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct EffectRef {
    pub name: String,
    pub args: Vec<EffectArgRef>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum EffectArgRef {
    Type(super::TypeId),
    Path(Vec<String>),
    String(String),
    Int(String),
    Wildcard,
}

impl EffectRowRef {
    /// A local inference variable, owned by one lambda occurrence. It is never
    /// a declared bound or evidence that the callable is pure.
    pub fn inferred_lambda(value: etas_hir::HirExprId) -> Self {
        Self {
            effects: Vec::new(),
            tail: Some(format!("@lambda:{}", value.0)),
        }
    }

    pub fn is_inferred_lambda(&self) -> bool {
        self.tail
            .as_ref()
            .is_some_and(|tail| tail.starts_with("@lambda:"))
    }
}
