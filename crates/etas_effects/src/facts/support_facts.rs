use std::collections::HashMap;

use etas_hir::HirItemId;

use crate::InterpreterSupport;

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct InterpreterSupportFacts {
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub items: HashMap<HirItemId, InterpreterSupport>,
    pub entry: Option<InterpreterSupport>,
}
