use std::collections::HashMap;

use etas_hir::{HirItemId, SymbolId};

use crate::RequirementSet;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequirementFacts {
    pub items: HashMap<HirItemId, RequirementSet>,
    pub symbols: HashMap<SymbolId, RequirementSet>,
}
