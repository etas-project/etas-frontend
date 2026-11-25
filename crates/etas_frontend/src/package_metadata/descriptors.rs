use std::collections::BTreeSet;

use super::{PackageMetadataError, projection::ProjectMetadataProjection};
use crate::{
    ModuleOrigin,
    code::{
        definition::DefinitionCatalog,
        identity::{DefinitionIdentityIndex, DefinitionOrigins},
    },
};
use etas_code::descriptor::{DefinitionDescriptorSet, DescriptorLimits};

impl ProjectMetadataProjection<'_> {
    pub(super) fn exported_definition_items(
        &self,
    ) -> Result<BTreeSet<etas_hir::HirItemId>, PackageMetadataError> {
        let mut selection = BTreeSet::new();
        for (_, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            for export in module.visibility_exports.items.values() {
                if export.visibility == etas_hir::Visibility::Public {
                    let item = self.hir_item_for_export(export)?;
                    selection.insert(item);
                    if let Some(group) = self
                        .checked
                        .items
                        .get(item)
                        .and_then(etas_hir::HirItem::symbol)
                        .and_then(|symbol| self.checked.hir.flow_overload_group(symbol))
                    {
                        for symbol in group {
                            if let Some(etas_hir::SymbolDef::Item { item }) =
                                self.checked.symbols.get(*symbol).map(|symbol| &symbol.def)
                            {
                                selection.insert(*item);
                            }
                        }
                    }
                }
            }
        }
        Ok(selection)
    }

    pub(super) fn agent_surface_items(
        &self,
        identities: &DefinitionIdentityIndex,
    ) -> Result<BTreeSet<etas_hir::HirItemId>, PackageMetadataError> {
        let mut tools = BTreeSet::new();
        for item in self.exported_definition_items()? {
            if matches!(
                self.checked.items.get(item),
                Some(etas_hir::HirItem::Agent(_))
            ) {
                tools.extend(
                    crate::code::definition::declared_tools(self.checked, identities, item)
                        .map_err(|error| PackageMetadataError::InvalidMetadataType {
                            reason: error.to_string(),
                        })?,
                );
            }
        }
        Ok(self
            .checked
            .items
            .iter()
            .filter_map(|(item, declaration)| {
                let etas_hir::HirItem::Tool(tool) = declaration else {
                    return None;
                };
                identities
                    .symbol(tool.symbol)
                    .is_some_and(|identity| tools.contains(identity))
                    .then_some(item)
            })
            .collect())
    }

    pub(super) fn definition_descriptors(
        &self,
        origins: &DefinitionOrigins,
        identities: DefinitionIdentityIndex,
    ) -> Result<DefinitionDescriptorSet, PackageMetadataError> {
        let limits = DescriptorLimits {
            max_nodes: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
            max_edges: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
            max_expansion_depth: etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
            max_text_bytes: etas_package_metadata::MAX_METADATA_UNCOMPRESSED_SECTION_SIZE,
        };
        let mut selection = self.exported_definition_items()?;
        selection.extend(self.agent_surface_items(&identities)?);
        selection.extend(
            self.checked
                .types
                .methods
                .entries()
                .filter(|method| self.internal_value_operation(method, Some(&identities)))
                .filter_map(|method| match method.target {
                    etas_types::method::MethodTarget::Source { callable, .. } => Some(callable),
                    _ => None,
                }),
        );
        let set = DefinitionCatalog::exported_descriptors(
            self.checked.clone(),
            origins,
            identities,
            &selection,
            limits,
        )
        .map_err(|error| PackageMetadataError::InvalidMetadataType {
            reason: error.to_string(),
        })?;
        etas_package_metadata::validate_definition_descriptors(&set).map_err(|error| {
            PackageMetadataError::InvalidMetadataType {
                reason: error.to_string(),
            }
        })?;
        Ok(set)
    }
}
