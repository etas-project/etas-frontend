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
        let mut selection = BTreeSet::new();
        for (_, module) in self.checked.module_index.modules.iter() {
            if !matches!(module.origin, ModuleOrigin::Source) {
                continue;
            }
            for export in module.visibility_exports.items.values() {
                if export.visibility == etas_hir::Visibility::Public {
                    selection.insert(self.hir_item_for_export(export)?);
                }
            }
        }
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
