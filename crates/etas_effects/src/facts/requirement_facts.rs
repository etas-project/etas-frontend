use std::collections::HashMap;

use etas_hir::{HirItemId, SymbolId};

use crate::RequirementSet;

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
pub struct RequirementFacts {
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub items: HashMap<HirItemId, RequirementSet>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub symbols: HashMap<SymbolId, RequirementSet>,
}
