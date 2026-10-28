use std::collections::BTreeSet;

use etas_hir::HirItemId;

use crate::{ModuleId, RuntimeSourceRequirements, UnitId};

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ReachabilityFacts {
    pub entry_item: Option<HirItemId>,
    pub reachable_items: BTreeSet<HirItemId>,
    pub reachable_bodies: BTreeSet<UnitId>,
    pub reachable_modules: BTreeSet<ModuleId>,
    pub analysis_items: BTreeSet<HirItemId>,
    pub analysis_bodies: BTreeSet<UnitId>,
    pub runtime_source_requirements: RuntimeSourceRequirements,
}
