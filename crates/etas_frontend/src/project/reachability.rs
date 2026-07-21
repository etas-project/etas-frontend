use std::collections::BTreeSet;

use etas_hir::HirItemId;

use crate::{ModuleId, RuntimeSourceRequirements, UnitId};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReachabilityFacts {
    pub entry_item: Option<HirItemId>,
    pub reachable_items: BTreeSet<HirItemId>,
    pub reachable_bodies: BTreeSet<UnitId>,
    pub reachable_modules: BTreeSet<ModuleId>,
    pub runtime_source_requirements: RuntimeSourceRequirements,
}
