use std::collections::{BTreeMap, BTreeSet};

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PipelineExecutionControl, PipelineInterruption, PreservedArtifacts,
};

use super::resolver::{
    CatalogModuleProvider, ModuleProvider, ModuleResolution, ProviderExport, ProviderExportTarget,
};
use crate::passes::artifacts::{MODULE_CATALOG, RESOLVED_IMPORTS, VALIDATED_EXTERNAL_ENVIRONMENT};
use crate::{
    ExternalPackageId, ImportTarget, ModuleCatalog, ModulePath, ProjectContext,
    ResolvedModuleTarget,
};

pub struct BindValidatedExternalImportTargetsPass;

impl Pass<ProjectContext> for BindValidatedExternalImportTargetsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BindValidatedExternalImportTargetsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                MODULE_CATALOG,
                RESOLVED_IMPORTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::from([MODULE_CATALOG, RESOLVED_IMPORTS]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        match bind(context, manager.execution_control()) {
            Ok(()) => PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::from([MODULE_CATALOG, RESOLVED_IMPORTS]),
            ),
            Err(BindingError::Invalid(reason)) => PassResult::failed(reason),
            Err(BindingError::Interrupted(interruption)) => PassResult::interrupted(interruption),
        }
    }
}

type ReExports = BTreeMap<(ExternalPackageId, Vec<String>), Vec<String>>;
type ResolvedTargets = BTreeMap<(ExternalPackageId, Vec<String>), ImportTarget>;

#[derive(Debug)]
enum BindingError {
    Invalid(String),
    Interrupted(PipelineInterruption),
}

impl From<String> for BindingError {
    fn from(reason: String) -> Self {
        Self::Invalid(reason)
    }
}

impl From<&str> for BindingError {
    fn from(reason: &str) -> Self {
        Self::Invalid(reason.into())
    }
}

fn checkpoint(control: Option<&dyn PipelineExecutionControl>) -> Result<(), BindingError> {
    match control {
        Some(control) => control.checkpoint().map_err(BindingError::Interrupted),
        None => Ok(()),
    }
}

fn bind(
    context: &mut ProjectContext,
    control: Option<&dyn PipelineExecutionControl>,
) -> Result<(), BindingError> {
    let environment = context
        .validated_external_environment
        .as_ref()
        .ok_or("external targets require validated metadata")?;
    let catalog = context
        .module_catalog
        .as_ref()
        .ok_or("external targets require a module catalog")?;
    let mut reexports = ReExports::new();
    for metadata in &environment.environment().external_public_metadata {
        for export in &metadata.re_exports {
            checkpoint(control)?;
            if reexports
                .insert(
                    (metadata.package, export.exported.clone()),
                    export.from.clone(),
                )
                .is_some_and(|previous| previous != export.from)
            {
                return Err("external export has conflicting checked origins".into());
            }
        }
    }
    let imports = context
        .resolved_imports
        .as_mut()
        .ok_or("external targets require resolved imports")?;
    let imports = std::sync::Arc::make_mut(imports);
    let provider = CatalogModuleProvider::new(catalog);
    let mut resolved_targets = ResolvedTargets::new();
    imports.module_members.clear();
    for import in &mut imports.imports {
        checkpoint(control)?;
        import.target = canonical_target(
            catalog,
            &reexports,
            &import.target,
            &mut resolved_targets,
            control,
        )?;
        if let ImportTarget::Module(module) = &import.target {
            let exports = provider
                .exports(module)
                .ok_or("module import has no resolved exports")?;
            let mut members = BTreeMap::new();
            for (name, export) in exports.items {
                checkpoint(control)?;
                if export.visibility != etas_hir::Visibility::Public {
                    continue;
                }
                let target = super::imports_common::import_target_for_export(&name, export);
                members.insert(
                    name,
                    canonical_target(catalog, &reexports, &target, &mut resolved_targets, control)?,
                );
            }
            imports.module_members.insert(module.key(), members);
        }
    }
    for import in &mut imports.wildcard_imports {
        checkpoint(control)?;
        if import.exported_names.len() != import.targets.len()
            || import
                .exported_names
                .iter()
                .any(|name| !import.targets.contains_key(name))
        {
            return Err("wildcard import is missing resolved export targets".into());
        }
        for target in import.targets.values_mut() {
            *target =
                canonical_target(catalog, &reexports, target, &mut resolved_targets, control)?;
        }
    }
    let mut updates = Vec::new();
    for (key, record) in catalog.modules.iter() {
        for (name, export) in &record.exports.items {
            checkpoint(control)?;
            let crate::ModuleExportTarget::External {
                package,
                module,
                module_path,
                symbol,
            } = &export.target
            else {
                continue;
            };
            let target = ImportTarget::ExternalItem {
                module_key: key,
                package: *package,
                module: *module,
                module_path: module_path.clone(),
                name: name.clone(),
                symbol: *symbol,
            };
            let canonical =
                canonical_target(catalog, &reexports, &target, &mut resolved_targets, control)?;
            if canonical != target {
                let target = super::imports::module_export_target_for_import(&canonical)
                    .ok_or("canonical export does not name a declaration")?;
                updates.push((key, name.clone(), target));
            }
        }
    }
    let catalog = context
        .module_catalog
        .as_mut()
        .ok_or("external targets lost their module catalog")?;
    for (key, name, target) in updates {
        checkpoint(control)?;
        let export = catalog
            .modules
            .get_mut(key)
            .and_then(|record| record.exports.items.get_mut(&name))
            .ok_or("canonical export lost its catalog record")?;
        export.target = target;
    }
    Ok(())
}

fn canonical_target(
    catalog: &ModuleCatalog,
    reexports: &ReExports,
    target: &ImportTarget,
    memo: &mut ResolvedTargets,
    control: Option<&dyn PipelineExecutionControl>,
) -> Result<ImportTarget, BindingError> {
    let mut current = target.clone();
    let mut visited = BTreeSet::new();
    let mut chain = Vec::new();
    while let ImportTarget::ExternalItem {
        package: Some(package),
        module_path,
        name,
        ..
    } = &current
    {
        checkpoint(control)?;
        let mut path = module_path.segments.clone();
        path.push(name.clone());
        let key = (*package, path);
        if let Some(resolved) = memo.get(&key) {
            current = resolved.clone();
            break;
        }
        let Some(origin) = reexports.get(&key) else {
            break;
        };
        if !visited.insert(key.clone()) {
            return Err("checked external re-export has a cyclic origin".into());
        }
        chain.push(key);
        current = declaring_export(catalog, origin)?;
    }
    for key in chain {
        memo.insert(key, current.clone());
    }
    Ok(current)
}

fn declaring_export(catalog: &ModuleCatalog, path: &[String]) -> Result<ImportTarget, String> {
    let (name, module) = path
        .split_last()
        .ok_or("external export has an empty declaration path")?;
    let provider = CatalogModuleProvider::new(catalog);
    let module = match provider.resolve_module(&ModulePath {
        segments: module.to_vec(),
    }) {
        ModuleResolution::Source { key, module } => ResolvedModuleTarget::Source { key, module },
        ModuleResolution::VirtualStd { key, module, path } => {
            ResolvedModuleTarget::Std { key, module, path }
        }
        ModuleResolution::External {
            key,
            package,
            module,
            path,
        } => ResolvedModuleTarget::External {
            key,
            package,
            module,
            path,
        },
        ModuleResolution::Missing | ModuleResolution::Ambiguous(_) => {
            return Err(format!(
                "external export `{}` has no unambiguous declaring module",
                path.join(".")
            ));
        }
    };
    let export: ProviderExport = provider.export(&module, name).ok_or_else(|| {
        format!(
            "external export `{}` has no declaration export",
            path.join(".")
        )
    })?;
    if export.visibility != etas_hir::Visibility::Public {
        return Err(format!(
            "external export `{}` refers to a private declaration",
            path.join(".")
        ));
    }
    if let ProviderExportTarget::Source { module, .. } = &export.target
        && catalog.source_module_key(*module).is_none()
    {
        return Err("external export has no source module identity".into());
    }
    Ok(super::imports_common::import_target_for_export(
        name, export,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Control {
        checks: AtomicUsize,
        cancel_at: Option<usize>,
    }
    impl PipelineExecutionControl for Control {
        fn checkpoint(&self) -> Result<(), PipelineInterruption> {
            let check = self.checks.fetch_add(1, Ordering::Relaxed);
            if self.cancel_at.is_some_and(|cancel_at| check >= cancel_at) {
                Err(PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "test cancellation".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    fn chain(length: u32) -> (ModuleCatalog, ReExports, ImportTarget) {
        let package = ExternalPackageId(7);
        let module = crate::ExternalModuleId(0);
        let path = ModulePath {
            segments: vec!["dep".into()],
        };
        let mut catalog = ModuleCatalog::default();
        let exports = (0..=length)
            .map(|i| {
                let name = format!("f{i}");
                (
                    name.clone(),
                    crate::ModuleExport {
                        name,
                        visibility: etas_hir::Visibility::Public,
                        target: crate::ModuleExportTarget::External {
                            package: Some(package),
                            module,
                            module_path: path.clone(),
                            symbol: crate::ExternalSymbolId(i),
                        },
                    },
                )
            })
            .collect();
        let key = catalog.insert(
            path.clone(),
            crate::CatalogModuleOrigin::ExternalMetadata {
                package: Some(package),
                module,
            },
            crate::ModuleExportTable { items: exports },
        );
        let reexports = (0..length)
            .map(|i| {
                (
                    (package, vec!["dep".into(), format!("f{i}")]),
                    vec!["dep".into(), format!("f{}", i + 1)],
                )
            })
            .collect();
        let target = ImportTarget::ExternalItem {
            module_key: key,
            package: Some(package),
            module,
            module_path: path,
            name: "f0".into(),
            symbol: crate::ExternalSymbolId(0),
        };
        (catalog, reexports, target)
    }

    #[test]
    fn reexport_chains_are_iterative_and_share_resolved_targets() {
        let (catalog, reexports, target) = chain(2_000);
        let control = Control {
            checks: AtomicUsize::new(0),
            cancel_at: None,
        };
        let mut memo = ResolvedTargets::new();
        let result =
            canonical_target(&catalog, &reexports, &target, &mut memo, Some(&control)).unwrap();
        assert!(matches!(&result, ImportTarget::ExternalItem { name, .. } if name == "f2000"));
        assert_eq!(memo.len(), 2_000);
        let checks = control.checks.load(Ordering::Relaxed);
        assert_eq!(
            canonical_target(&catalog, &reexports, &target, &mut memo, Some(&control)).unwrap(),
            result
        );
        assert_eq!(control.checks.load(Ordering::Relaxed), checks + 1);
    }

    #[test]
    fn reexport_binding_preserves_pipeline_cancellation() {
        let (catalog, reexports, target) = chain(2_000);
        let control = Control {
            checks: AtomicUsize::new(0),
            cancel_at: Some(3),
        };
        let mut memo = ResolvedTargets::new();
        assert!(matches!(
            canonical_target(&catalog, &reexports, &target, &mut memo, Some(&control)),
            Err(BindingError::Interrupted(PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                ..
            }))
        ));
        assert!(memo.is_empty());
    }
}
