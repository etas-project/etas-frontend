use std::collections::HashMap;

use crate::{
    AstItemRef, CatalogModuleOrigin, ModuleCatalog, ModuleExportTarget, ModuleId, ModuleKey,
    ModulePath, ResolvedModuleTarget,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleResolution {
    Source {
        key: ModuleKey,
        module: ModuleId,
    },
    VirtualStd {
        key: ModuleKey,
        module: etas_std::StdModuleId,
        path: ModulePath,
    },
    External {
        key: ModuleKey,
        package: Option<crate::ExternalPackageId>,
        module: crate::ExternalModuleId,
        path: ModulePath,
    },
    Missing,
    Ambiguous(Vec<ResolvedModuleTarget>),
}

pub trait ModuleProvider {
    fn resolve_module(&self, path: &ModulePath) -> ModuleResolution;
    fn exports(&self, target: &ResolvedModuleTarget) -> Option<ProviderExportTable>;
}

pub struct CatalogModuleProvider<'a> {
    catalog: &'a ModuleCatalog,
}

impl<'a> CatalogModuleProvider<'a> {
    pub fn new(catalog: &'a ModuleCatalog) -> Self {
        Self { catalog }
    }
}

impl ModuleProvider for CatalogModuleProvider<'_> {
    fn resolve_module(&self, path: &ModulePath) -> ModuleResolution {
        let Some(keys) = self.catalog.module_keys(path) else {
            return ModuleResolution::Missing;
        };
        let effective_keys = effective_catalog_keys(self.catalog, keys);
        let mut targets = Vec::new();
        for key in effective_keys {
            let Some(record) = self.catalog.modules.get(*key) else {
                continue;
            };
            targets.push(match &record.origin {
                CatalogModuleOrigin::ProjectSource {
                    module: source_module,
                } => ResolvedModuleTarget::Source {
                    key: *key,
                    module: *source_module,
                },
                CatalogModuleOrigin::DependencySourceOverlay {
                    module: source_module,
                    ..
                } => ResolvedModuleTarget::Source {
                    key: *key,
                    module: *source_module,
                },
                CatalogModuleOrigin::ExternalMetadata {
                    package,
                    module: external_module,
                } => ResolvedModuleTarget::External {
                    key: *key,
                    package: *package,
                    module: *external_module,
                    path: record.path.clone(),
                },
                CatalogModuleOrigin::Std { module: std_module } => ResolvedModuleTarget::Std {
                    key: *key,
                    module: *std_module,
                    path: record.path.clone(),
                },
            });
        }
        match targets.as_slice() {
            [] => ModuleResolution::Missing,
            [ResolvedModuleTarget::Source { key, module }] => ModuleResolution::Source {
                key: *key,
                module: *module,
            },
            [ResolvedModuleTarget::Std { key, module, path }] => ModuleResolution::VirtualStd {
                key: *key,
                module: *module,
                path: path.clone(),
            },
            [
                ResolvedModuleTarget::External {
                    key,
                    package,
                    module,
                    path,
                },
            ] => ModuleResolution::External {
                key: *key,
                package: *package,
                module: *module,
                path: path.clone(),
            },
            targets => ModuleResolution::Ambiguous(targets.to_vec()),
        }
    }

    fn exports(&self, target: &ResolvedModuleTarget) -> Option<ProviderExportTable> {
        let record = self.catalog.modules.get(target.key())?;
        Some(ProviderExportTable {
            items: record
                .exports
                .items
                .iter()
                .map(|(name, export)| {
                    (
                        name.clone(),
                        ProviderExport {
                            name: export.name.clone(),
                            visibility: export.visibility,
                            target: match &export.target {
                                ModuleExportTarget::Source { module, item } => {
                                    ProviderExportTarget::Source {
                                        module_key: record.key,
                                        module: *module,
                                        item: item.clone(),
                                    }
                                }
                                ModuleExportTarget::Std {
                                    module,
                                    module_path,
                                    symbol,
                                } => ProviderExportTarget::Std {
                                    module_key: record.key,
                                    module: *module,
                                    module_path: module_path.clone(),
                                    symbol: *symbol,
                                },
                                ModuleExportTarget::External {
                                    package,
                                    module,
                                    module_path,
                                    symbol,
                                } => ProviderExportTarget::External {
                                    module_key: record.key,
                                    package: *package,
                                    module: *module,
                                    module_path: module_path.clone(),
                                    symbol: *symbol,
                                },
                            },
                        },
                    )
                })
                .collect(),
        })
    }
}

fn effective_catalog_keys<'a>(
    catalog: &ModuleCatalog,
    keys: &'a [ModuleKey],
) -> Vec<&'a ModuleKey> {
    let overlay_keys = keys
        .iter()
        .filter(|key| {
            catalog.modules.get(**key).is_some_and(|record| {
                matches!(
                    &record.origin,
                    CatalogModuleOrigin::DependencySourceOverlay { .. }
                )
            })
        })
        .collect::<Vec<_>>();
    if !overlay_keys.is_empty() {
        return overlay_keys;
    }
    keys.iter().collect()
}

#[derive(Clone, Debug, Default)]
pub struct ProviderExportTable {
    pub items: HashMap<String, ProviderExport>,
}

#[derive(Clone, Debug)]
pub struct ProviderExport {
    pub name: String,
    pub visibility: etas_hir::Visibility,
    pub target: ProviderExportTarget,
}

#[derive(Clone, Debug)]
pub enum ProviderExportTarget {
    Source {
        module_key: ModuleKey,
        module: ModuleId,
        item: AstItemRef,
    },
    Std {
        module_key: ModuleKey,
        module: etas_std::StdModuleId,
        module_path: ModulePath,
        symbol: etas_std::StdSymbolId,
    },
    External {
        module_key: ModuleKey,
        package: Option<crate::ExternalPackageId>,
        module: crate::ExternalModuleId,
        module_path: ModulePath,
        symbol: crate::ExternalSymbolId,
    },
}

pub struct ProjectModuleResolver<'a> {
    providers: Vec<&'a dyn ModuleProvider>,
}

impl Default for ProjectModuleResolver<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ProjectModuleResolver<'a> {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn with_provider(mut self, provider: &'a dyn ModuleProvider) -> Self {
        self.providers.push(provider);
        self
    }

    pub fn resolve_module(&self, path: &ModulePath) -> ModuleResolution {
        let mut resolved = Vec::new();
        for provider in &self.providers {
            match provider.resolve_module(path) {
                ModuleResolution::Missing => {}
                ModuleResolution::Source { key, module } => {
                    resolved.push(ResolvedModuleTarget::Source { key, module });
                }
                ModuleResolution::VirtualStd { key, module, path } => {
                    resolved.push(ResolvedModuleTarget::Std { key, module, path });
                }
                ModuleResolution::External {
                    key,
                    package,
                    module,
                    path,
                } => {
                    resolved.push(ResolvedModuleTarget::External {
                        key,
                        package,
                        module,
                        path,
                    });
                }
                ModuleResolution::Ambiguous(targets) => {
                    return ModuleResolution::Ambiguous(targets);
                }
            }
        }
        match resolved.as_slice() {
            [] => ModuleResolution::Missing,
            [ResolvedModuleTarget::Source { key, module }] => ModuleResolution::Source {
                key: *key,
                module: *module,
            },
            [ResolvedModuleTarget::Std { key, module, path }] => ModuleResolution::VirtualStd {
                key: *key,
                module: *module,
                path: path.clone(),
            },
            [
                ResolvedModuleTarget::External {
                    key,
                    package,
                    module,
                    path,
                },
            ] => ModuleResolution::External {
                key: *key,
                package: *package,
                module: *module,
                path: path.clone(),
            },
            targets => ModuleResolution::Ambiguous(targets.to_vec()),
        }
    }

    pub fn exports(&self, target: &ResolvedModuleTarget) -> Option<ProviderExportTable> {
        self.providers
            .iter()
            .find_map(|provider| provider.exports(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CatalogModuleOrigin, ExternalModuleId, ModuleCatalog, ModuleExportTable, ModuleId,
    };

    #[test]
    fn catalog_provider_resolves_declared_external_module() {
        let path = module_path(&["company", "agents", "writer"]);
        let mut catalog = ModuleCatalog::default();
        let key = catalog.insert(
            path.clone(),
            CatalogModuleOrigin::ExternalMetadata {
                package: None,
                module: ExternalModuleId(2),
            },
            ModuleExportTable::default(),
        );
        let provider = CatalogModuleProvider::new(&catalog);

        assert_eq!(
            provider.resolve_module(&path),
            ModuleResolution::External {
                key,
                package: None,
                module: ExternalModuleId(2),
                path
            }
        );
    }

    #[test]
    fn catalog_provider_rejects_duplicate_external_module_path() {
        let path = module_path(&["company", "agents", "writer"]);
        let mut catalog = ModuleCatalog::default();
        let first = catalog.insert(
            path.clone(),
            CatalogModuleOrigin::ExternalMetadata {
                package: None,
                module: ExternalModuleId(1),
            },
            ModuleExportTable::default(),
        );
        let second = catalog.insert(
            path.clone(),
            CatalogModuleOrigin::ExternalMetadata {
                package: None,
                module: ExternalModuleId(2),
            },
            ModuleExportTable::default(),
        );
        let provider = CatalogModuleProvider::new(&catalog);

        assert!(matches!(
            provider.resolve_module(&path),
            ModuleResolution::Ambiguous(targets)
                if targets
                    == vec![
                        ResolvedModuleTarget::External {
                            key: first,
                            package: None,
                            module: ExternalModuleId(1),
                            path: path.clone(),
                        },
                        ResolvedModuleTarget::External {
                            key: second,
                            package: None,
                            module: ExternalModuleId(2),
                            path: path.clone(),
                        },
                    ]
        ));
    }

    #[test]
    fn catalog_provider_rejects_source_external_module_conflict() {
        let path = module_path(&["company", "agents", "writer"]);
        let mut catalog = ModuleCatalog::default();
        let source = catalog.insert(
            path.clone(),
            CatalogModuleOrigin::ProjectSource {
                module: ModuleId(7),
            },
            ModuleExportTable::default(),
        );
        let external = catalog.insert(
            path.clone(),
            CatalogModuleOrigin::ExternalMetadata {
                package: None,
                module: ExternalModuleId(2),
            },
            ModuleExportTable::default(),
        );
        let provider = CatalogModuleProvider::new(&catalog);

        assert!(matches!(
            provider.resolve_module(&path),
            ModuleResolution::Ambiguous(targets)
                if targets
                    == vec![
                        ResolvedModuleTarget::Source {
                            key: source,
                            module: ModuleId(7),
                        },
                        ResolvedModuleTarget::External {
                            key: external,
                            package: None,
                            module: ExternalModuleId(2),
                            path,
                        },
                    ]
        ));
    }

    fn module_path(segments: &[&str]) -> ModulePath {
        ModulePath {
            segments: segments
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        }
    }
}
