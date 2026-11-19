use std::collections::{BTreeMap, HashMap};

use etas_code::{
    PackageKey,
    identity::{DefinitionBindingOrigin, DefinitionModuleOrigin, PackageDefinitionOrigins},
};

use crate::{
    ModuleOrigin,
    code::identity::{DefinitionIdentityIndex, DefinitionOrigins},
};

use super::{PackageMetadataError, projection::ProjectMetadataProjection};

impl ProjectMetadataProjection<'_> {
    pub(super) fn definition_identity_input(
        &self,
        package: PackageKey,
    ) -> Result<(DefinitionOrigins, DefinitionIdentityIndex), PackageMetadataError> {
        let invalid = |error: crate::code::descriptor::ProjectionError| {
            PackageMetadataError::InvalidMetadataType {
                reason: error.to_string(),
            }
        };
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(self.checked, package)
                .map_err(invalid)?,
            standard_package: crate::code::identity::standard_package_key(
                &self.checked.std_registry,
            )
            .map_err(invalid)?,
            external_definitions: DefinitionOrigins::checked_external_definitions(self.checked)
                .map_err(invalid)?,
        };
        let identities = DefinitionIdentityIndex::build(self.checked, &origins).map_err(invalid)?;
        Ok((origins, identities))
    }

    pub(super) fn definition_origins(
        &self,
        package: PackageKey,
        origins: &DefinitionOrigins,
        identities: &DefinitionIdentityIndex,
    ) -> Result<PackageDefinitionOrigins, PackageMetadataError> {
        let invalid = |error: crate::code::descriptor::ProjectionError| {
            PackageMetadataError::InvalidMetadataType {
                reason: error.to_string(),
            }
        };
        let mut module_origins = HashMap::new();
        for (module, origin) in &origins.source_modules {
            let source = self
                .checked
                .modules_arena
                .get(*module)
                .ok_or_else(|| {
                    invalid(crate::code::descriptor::ProjectionError::MissingModule(
                        *module,
                    ))
                })?
                .span
                .source;
            let module = self
                .checked
                .module_index
                .by_source
                .get(&source)
                .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                    reason: "definition source has no indexed module".into(),
                })?;
            if module_origins.insert(*module, origin).is_some() {
                return Err(PackageMetadataError::InvalidMetadataType {
                    reason: "duplicate canonical module origin".into(),
                });
            }
        }
        let mut modules = Vec::new();
        let mut declarations = BTreeMap::new();
        for (module_id, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            let origin = module_origins.get(&module_id).ok_or_else(|| {
                PackageMetadataError::InvalidMetadataType {
                    reason: "source module has no canonical definition origin".into(),
                }
            })?;
            modules.push(DefinitionModuleOrigin {
                path: module.path.segments.clone(),
                canonical_path: origin.module().to_vec(),
            });
            for export in module.visibility_exports.items.values() {
                let item = self.hir_item_for_export(export)?;
                let symbol = self
                    .checked
                    .items
                    .get(item)
                    .and_then(etas_hir::HirItem::symbol)
                    .ok_or_else(|| PackageMetadataError::MissingExportSignature {
                        path: module.path.segments.clone(),
                    })?;
                let definition = identities
                    .symbol(symbol)
                    .ok_or_else(|| {
                        invalid(
                            crate::code::descriptor::ProjectionError::MissingDeclaration(symbol),
                        )
                    })?
                    .clone();
                let binding = DefinitionBindingOrigin {
                    module: module.path.segments.clone(),
                    declaration: vec![export.name.clone()],
                    definition: definition.clone(),
                };
                if declarations.insert(binding.path(), binding).is_some() {
                    return Err(PackageMetadataError::InvalidMetadataType {
                        reason: "duplicate definition export binding".into(),
                    });
                }
                if let Some(group) = self.checked.hir.flow_overload_group(symbol) {
                    for member in group {
                        let definition = identities
                            .symbol(*member)
                            .ok_or_else(|| {
                                invalid(
                                    crate::code::descriptor::ProjectionError::MissingDeclaration(
                                        *member,
                                    ),
                                )
                            })?
                            .clone();
                        let binding = DefinitionBindingOrigin {
                            module: definition.module().to_vec(),
                            declaration: definition.declaration().to_vec(),
                            definition,
                        };
                        declarations.entry(binding.path()).or_insert(binding);
                    }
                }
                if definition.module() == origin.module()
                    && definition.declaration() == [export.name.clone()]
                    && let Some(etas_hir::HirItem::Enum(owner)) = self.checked.items.get(item)
                {
                    for variant in &owner.variants {
                        let definition = identities
                            .symbol(variant.symbol)
                            .ok_or_else(|| {
                                invalid(
                                    crate::code::descriptor::ProjectionError::MissingDeclaration(
                                        variant.symbol,
                                    ),
                                )
                            })?
                            .clone();
                        let binding = DefinitionBindingOrigin {
                            module: module.path.segments.clone(),
                            declaration: vec![
                                export.name.clone(),
                                self.checked
                                    .symbols
                                    .get(variant.symbol)
                                    .unwrap()
                                    .name
                                    .clone(),
                            ],
                            definition,
                        };
                        declarations.insert(binding.path(), binding);
                    }
                }
            }
        }
        for export in self.external_re_exports()? {
            let module = self
                .checked
                .module_index
                .modules
                .get(export.module)
                .ok_or_else(|| PackageMetadataError::MissingExportSignature {
                    path: export.exported.clone(),
                })?;
            let definition = identities
                .path(&export.from)
                .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                    reason: format!(
                        "re-export `{}` has no checked canonical definition origin",
                        export.exported.join(".")
                    ),
                })?
                .clone();
            let binding = DefinitionBindingOrigin {
                module: module.path.segments.clone(),
                declaration: vec![export.name],
                definition,
            };
            if declarations.insert(binding.path(), binding).is_some() {
                return Err(PackageMetadataError::InvalidMetadataType {
                    reason: "duplicate external definition export binding".into(),
                });
            }
        }
        modules.sort_by(|a, b| a.path.cmp(&b.path));
        let origins = PackageDefinitionOrigins {
            package,
            modules,
            declarations: declarations.into_values().collect(),
        };
        origins
            .validate()
            .map_err(|error| PackageMetadataError::InvalidMetadataType {
                reason: error.to_string(),
            })?;
        Ok(origins)
    }
}
