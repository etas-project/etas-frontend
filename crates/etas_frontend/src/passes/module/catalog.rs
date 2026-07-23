use std::collections::HashMap;

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{MODULE_CATALOG, MODULE_INDEX};
use crate::{
    CatalogModuleOrigin, ModuleCatalog, ModuleExport, ModuleExportTable, ModuleExportTarget,
    ModuleOrigin, ProjectContext,
};

pub struct BuildModuleCatalogPass;

impl Pass<ProjectContext> for BuildModuleCatalogPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildModuleCatalogPass", PassKind::Analysis)
            .requires(ArtifactSet::one(MODULE_INDEX))
            .produces(ArtifactSet::one(MODULE_CATALOG))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let modules = context.modules.as_ref().expect("module index should exist");
        let mut catalog = ModuleCatalog::default();

        for (module_id, module) in modules.modules.iter() {
            let exports = ModuleExportTable {
                items: module
                    .visibility_exports
                    .items
                    .iter()
                    .map(|(name, item)| {
                        (
                            name.clone(),
                            ModuleExport {
                                name: item.name.clone(),
                                visibility: item.visibility,
                                target: ModuleExportTarget::Source {
                                    module: module_id,
                                    item: item.item.clone(),
                                },
                            },
                        )
                    })
                    .collect(),
            };
            let origin = match &module.origin {
                ModuleOrigin::DependencySourceOverlay { package, .. } => {
                    CatalogModuleOrigin::DependencySourceOverlay {
                        package: *package,
                        module: module_id,
                    }
                }
                _ => CatalogModuleOrigin::ProjectSource { module: module_id },
            };
            catalog.insert(module.path.clone(), origin, exports);
        }

        if !context
            .input
            .environment
            .external_packages
            .iter()
            .any(|package| package.import_root == "std")
        {
            add_std_modules(&mut catalog, &context.std_registry);
        }
        add_external_modules(&mut catalog, context);

        context.module_catalog = Some(catalog);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(MODULE_CATALOG))
    }
}

fn add_std_modules(catalog: &mut ModuleCatalog, registry: &etas_std::StdRegistry) {
    for module in registry.modules() {
        let path = crate::ModulePath {
            segments: module.path.clone(),
        };
        let items = registry
            .symbols()
            .filter(|symbol| symbol.module == module.id)
            .map(|symbol| {
                (
                    symbol.name.clone(),
                    ModuleExport {
                        name: symbol.name.clone(),
                        visibility: etas_hir::Visibility::Public,
                        target: ModuleExportTarget::Std {
                            module: module.id,
                            module_path: path.clone(),
                            symbol: symbol.id,
                        },
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        catalog.insert(
            path,
            CatalogModuleOrigin::Std { module: module.id },
            ModuleExportTable { items },
        );
    }
}

fn add_external_modules(catalog: &mut ModuleCatalog, context: &ProjectContext) {
    for module in &context.input.environment.external_modules {
        let exports = ModuleExportTable {
            items: module
                .exports
                .iter()
                .map(|export| {
                    (
                        export.name.clone(),
                        ModuleExport {
                            name: export.name.clone(),
                            visibility: export.visibility,
                            target: ModuleExportTarget::External {
                                package: module.package,
                                module: module.id,
                                module_path: module.path.clone(),
                                symbol: export.symbol,
                            },
                        },
                    )
                })
                .collect(),
        };
        catalog.insert(
            module.path.clone(),
            CatalogModuleOrigin::ExternalMetadata {
                package: module.package,
                module: module.id,
            },
            exports,
        );
    }
}
