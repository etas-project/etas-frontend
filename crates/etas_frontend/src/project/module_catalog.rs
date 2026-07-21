use std::collections::{BTreeMap, HashMap};

use etas_core::{Arena, id_type};

use crate::{
    AstItemRef, ExternalModuleId, ExternalPackageId, ExternalSymbolId, ModuleId, ModulePath,
};

id_type!(ModuleKey);

#[derive(Clone, Debug, Default)]
pub struct ModuleCatalog {
    pub modules: Arena<ModuleKey, ModuleRecord>,
    pub by_path: HashMap<ModulePath, Vec<ModuleKey>>,
    pub namespace: ModuleNamespaceTree,
}

impl ModuleCatalog {
    pub fn insert(
        &mut self,
        path: ModulePath,
        origin: ModuleOrigin,
        exports: ModuleExportTable,
    ) -> ModuleKey {
        let kind = origin.kind();
        let key = self.modules.alloc_with_id(|key| ModuleRecord {
            key,
            path: path.clone(),
            origin,
            kind,
            exports,
        });
        self.by_path.entry(path.clone()).or_default().push(key);
        self.namespace.insert(&path, key);
        key
    }

    pub fn module_keys(&self, path: &ModulePath) -> Option<&[ModuleKey]> {
        self.namespace.module_keys(path)
    }

    pub fn source_module_key(&self, module: ModuleId) -> Option<ModuleKey> {
        self.modules
            .iter()
            .find_map(|(key, record)| match &record.origin {
                ModuleOrigin::ProjectSource { module: candidate }
                | ModuleOrigin::DependencySourceOverlay {
                    module: candidate, ..
                } if *candidate == module => Some(key),
                _ => None,
            })
    }
}

#[derive(Clone, Debug)]
pub struct ModuleRecord {
    pub key: ModuleKey,
    pub path: ModulePath,
    pub origin: ModuleOrigin,
    pub kind: ModuleOriginKind,
    pub exports: ModuleExportTable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleOriginKind {
    ProjectSource,
    DependencySourceOverlay,
    ExternalMetadata,
    Std,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleOrigin {
    ProjectSource {
        module: ModuleId,
    },
    DependencySourceOverlay {
        package: ExternalPackageId,
        module: ModuleId,
    },
    ExternalMetadata {
        package: Option<ExternalPackageId>,
        module: ExternalModuleId,
    },
    Std {
        module: etas_std::StdModuleId,
    },
}

impl ModuleOrigin {
    fn kind(&self) -> ModuleOriginKind {
        match self {
            Self::ProjectSource { .. } => ModuleOriginKind::ProjectSource,
            Self::DependencySourceOverlay { .. } => ModuleOriginKind::DependencySourceOverlay,
            Self::ExternalMetadata { .. } => ModuleOriginKind::ExternalMetadata,
            Self::Std { .. } => ModuleOriginKind::Std,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleExportTable {
    pub items: HashMap<String, ModuleExport>,
}

#[derive(Clone, Debug)]
pub struct ModuleExport {
    pub name: String,
    pub visibility: etas_hir::Visibility,
    pub target: ModuleExportTarget,
}

#[derive(Clone, Debug)]
pub enum ModuleExportTarget {
    Source {
        module: ModuleId,
        item: AstItemRef,
    },
    Std {
        module: etas_std::StdModuleId,
        module_path: ModulePath,
        symbol: etas_std::StdSymbolId,
    },
    External {
        package: Option<ExternalPackageId>,
        module: ExternalModuleId,
        module_path: ModulePath,
        symbol: ExternalSymbolId,
    },
}

#[derive(Clone, Debug, Default)]
pub struct ModuleNamespaceTree {
    pub root: ModuleNamespaceNode,
}

impl ModuleNamespaceTree {
    pub fn insert(&mut self, path: &ModulePath, key: ModuleKey) {
        let mut node = &mut self.root;
        for segment in &path.segments {
            node = node.children.entry(segment.clone()).or_default();
        }
        node.modules.push(key);
    }

    pub fn module_keys(&self, path: &ModulePath) -> Option<&[ModuleKey]> {
        let mut node = &self.root;
        for segment in &path.segments {
            node = node.children.get(segment)?;
        }
        (!node.modules.is_empty()).then_some(node.modules.as_slice())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleNamespaceNode {
    pub modules: Vec<ModuleKey>,
    pub children: BTreeMap<String, ModuleNamespaceNode>,
}
