use std::collections::{BTreeMap, BTreeSet};

use crate::{ExternalPackageId, ModulePath};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeSourceRequirements {
    pub dependencies: BTreeMap<ExternalPackageId, RuntimeSourceRequirement>,
}

impl RuntimeSourceRequirements {
    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSourceRequirement {
    pub package: ExternalPackageId,
    pub import_root: String,
    pub seed_modules: BTreeSet<ModulePath>,
    pub required_modules: BTreeSet<ModulePath>,
    pub reasons: Vec<RuntimeSourceReason>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSourceReason {
    pub item_path: Vec<String>,
    pub module_path: ModulePath,
    pub kind: RuntimeSourceReasonKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeSourceReasonKind {
    ExternalRuntimeItem,
    ExternalRuntimeModule,
}
