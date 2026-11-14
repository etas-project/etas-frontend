use std::collections::HashMap;

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::passes::artifacts::{MODULE_CATALOG, MODULE_INDEX, global_with_diagnostics};
use crate::{
    CatalogModuleOrigin, ModuleCatalog, ModuleExport, ModuleExportTable, ModuleExportTarget,
    ModuleOrigin, ProjectContext,
};

pub struct BuildModuleCatalogPass;

impl Pass<ProjectContext> for BuildModuleCatalogPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildModuleCatalogPass", PassKind::Analysis)
            .requires(ArtifactSet::one(MODULE_INDEX))
            .produces(global_with_diagnostics([MODULE_CATALOG]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let modules = context.modules.as_ref().expect("module index should exist");
        if context.input.options.standard_library_mode
            != crate::StandardLibraryMode::SourceBootstrap
            && let Some((_, module)) = modules.modules.iter().find(|(_, module)| {
                module
                    .path
                    .segments
                    .first()
                    .is_some_and(|root| root == "std")
                    && !matches!(module.origin, ModuleOrigin::VirtualStd { .. })
            })
        {
            context.diagnostics.push(etas_core::Diagnostic::syntax(
                etas_core::SyntaxDiagnosticCode::InvalidItem,
                module.span,
                format!(
                    "module `{}` conflicts with the toolchain-owned `std` root",
                    module.path.segments.join(".")
                ),
            ));
            return PassResult::stop();
        }
        if let Some(package) = context
            .input
            .environment
            .external_packages
            .iter()
            .find(|package| package.import_root.split('.').next() == Some("std"))
        {
            let span = context.input.sources.first().map_or_else(
                || etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO),
                |source| etas_core::Span::empty(source.id, etas_core::TextSize::ZERO),
            );
            context.diagnostics.push(etas_core::Diagnostic::syntax(
                etas_core::SyntaxDiagnosticCode::InvalidItem, span,
                format!("dependency `{}` import root `{}` conflicts with the toolchain-owned `std` root", package.name, package.import_root),
            ));
            return PassResult::stop();
        }
        let mut catalog = ModuleCatalog {
            dependency_namespaces: modules.dependency_namespaces.clone(),
            ..ModuleCatalog::default()
        };

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
                ModuleOrigin::VirtualStd { source, .. } => CatalogModuleOrigin::StdSource {
                    module: module_id,
                    source: *source,
                },
                _ => CatalogModuleOrigin::ProjectSource { module: module_id },
            };
            catalog.insert(module.path.clone(), origin, exports);
        }

        add_std_modules(
            &mut catalog,
            &context.std_registry,
            context.input.options.standard_library_mode,
        );
        add_external_modules(&mut catalog, context);

        context.module_catalog = Some(catalog);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(MODULE_CATALOG))
    }
}

fn add_std_modules(
    catalog: &mut ModuleCatalog,
    registry: &etas_std::StdRegistry,
    mode: crate::StandardLibraryMode,
) {
    for module in registry.modules() {
        let path = crate::ModulePath {
            segments: module.path.clone(),
        };
        let items = registry
            .module_exports(module.id)
            .filter(|(_, symbol)| {
                mode != crate::StandardLibraryMode::SourceBootstrap
                    || !matches!(
                        symbol.implementation,
                        Some(etas_std::StdImplementationRef::Source { .. })
                    )
            })
            .map(|(name, symbol)| {
                (
                    name.to_owned(),
                    ModuleExport {
                        name: name.to_owned(),
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
        if mode == crate::StandardLibraryMode::SourceBootstrap
            && let Some(key) = catalog
                .module_keys(&path)
                .and_then(|keys| keys.first())
                .copied()
        {
            let source = catalog.modules.get_mut(key).expect("source std module");
            for (name, export) in items {
                source.exports.items.entry(name).or_insert(export);
            }
            continue;
        }
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
