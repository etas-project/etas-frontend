use super::*;

pub(in crate::package_metadata) struct ExternalReExport {
    pub module: crate::ModuleId,
    pub name: String,
    pub from: Vec<String>,
    pub exported: Vec<String>,
}

impl ProjectMetadataProjection<'_> {
    pub(in crate::package_metadata) fn external_re_exports(
        &self,
    ) -> Result<Vec<ExternalReExport>, PackageMetadataError> {
        let mut exports = BTreeMap::new();
        let explicit = self
            .checked
            .resolved_imports
            .imports
            .iter()
            .filter(|import| import.visibility == Visibility::Public)
            .map(|import| (import.from, import.local_name.as_str(), &import.target));
        let wildcard = self
            .checked
            .resolved_imports
            .wildcard_imports
            .iter()
            .filter(|import| import.visibility == Visibility::Public)
            .flat_map(|import| {
                import
                    .targets
                    .iter()
                    .map(move |(name, target)| (import.from, name.as_str(), target))
            });
        for (module, name, target) in explicit.chain(wildcard) {
            let from = match target {
                crate::ImportTarget::ExternalItem {
                    module_path, name, ..
                }
                | crate::ImportTarget::StdItem {
                    module_path, name, ..
                } => module_path
                    .segments
                    .iter()
                    .cloned()
                    .chain(std::iter::once(name.clone()))
                    .collect(),
                crate::ImportTarget::SourceItem { .. } | crate::ImportTarget::Module(_) => continue,
            };
            let declaration = self
                .checked
                .module_index
                .modules
                .get(module)
                .ok_or_else(|| PackageMetadataError::MissingExportSignature {
                    path: vec![name.into()],
                })?;
            if !matches!(declaration.origin, ModuleOrigin::Source) {
                continue;
            }
            let mut exported = declaration.path.segments.clone();
            exported.push(name.to_owned());
            let record = ExternalReExport {
                module,
                name: name.into(),
                from,
                exported: exported.clone(),
            };
            if let Some(previous) = exports.insert(exported, record)
                && previous.from != exports[&previous.exported].from
            {
                return Err(PackageMetadataError::InvalidMetadataType {
                    reason: "resolved public export has conflicting declaration origins".into(),
                });
            }
        }
        Ok(exports.into_values().collect())
    }
}
