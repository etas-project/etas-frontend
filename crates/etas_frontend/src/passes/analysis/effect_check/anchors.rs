use std::collections::{BTreeMap, HashMap, HashSet};

use etas_core::Span;

use crate::{ExternalPackageId, ImportTarget, ProjectInput, ResolvedImports, ResolvedModuleTarget};

#[derive(Clone, Debug, Default)]
pub(super) struct ExternalImportAnchorIndex {
    package_spans: BTreeMap<ExternalPackageId, Span>,
    item_spans: BTreeMap<(ExternalPackageId, Vec<String>), Span>,
    module_spans: BTreeMap<(ExternalPackageId, Vec<String>), Span>,
}

impl ExternalImportAnchorIndex {
    pub(super) fn build(imports: &ResolvedImports) -> Self {
        let mut index = Self::default();
        for import in &imports.imports {
            match &import.target {
                ImportTarget::ExternalItem {
                    package: Some(package),
                    module_path,
                    name,
                    ..
                } => {
                    let mut item = module_path.segments.clone();
                    item.push(name.clone());
                    index.insert_item(*package, item, import.span);
                    index.insert_module(*package, module_path.segments.clone(), import.span);
                }
                ImportTarget::Module(ResolvedModuleTarget::External {
                    package: Some(package),
                    path,
                    ..
                }) => index.insert_module(*package, path.segments.clone(), import.span),
                _ => {}
            }
        }
        for import in &imports.wildcard_imports {
            let ResolvedModuleTarget::External {
                package: Some(package),
                path,
                ..
            } = &import.target_module
            else {
                continue;
            };
            index.insert_module(*package, path.segments.clone(), import.span);
            for name in &import.exported_names {
                let mut item = path.segments.clone();
                item.push(name.clone());
                index.insert_item(*package, item, import.span);
            }
        }
        index
    }

    fn insert_item(&mut self, package: ExternalPackageId, item: Vec<String>, span: Span) {
        self.package_spans.entry(package).or_insert(span);
        self.item_spans.entry((package, item)).or_insert(span);
    }

    fn insert_module(&mut self, package: ExternalPackageId, module: Vec<String>, span: Span) {
        self.package_spans.entry(package).or_insert(span);
        self.module_spans.entry((package, module)).or_insert(span);
    }

    pub(super) fn package_span(&self, package: ExternalPackageId) -> Option<Span> {
        self.package_spans.get(&package).copied()
    }

    pub(super) fn item_span(&self, package: ExternalPackageId, item: &[String]) -> Option<Span> {
        self.item_spans
            .get(&(package, item.to_vec()))
            .copied()
            .or_else(|| {
                self.module_spans
                    .iter()
                    .filter(|((candidate_package, module), _)| {
                        *candidate_package == package && item.starts_with(module)
                    })
                    .max_by_key(|((_, module), _)| module.len())
                    .map(|(_, span)| *span)
            })
    }
}

pub(super) fn external_artifact_anchors(
    input: &ProjectInput,
    anchors: &ExternalImportAnchorIndex,
) -> Vec<etas_effects::ExternalArtifactAnchor> {
    let packages = input
        .environment
        .external_packages
        .iter()
        .map(|package| (package.id, package))
        .collect::<HashMap<_, _>>();
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for metadata in &input.environment.external_public_metadata {
        let Some(package) = packages.get(&metadata.package) else {
            continue;
        };
        if anchors.package_span(metadata.package).is_none() {
            continue;
        }
        let package_identity = format!(
            "{}@{}#{}",
            package.name, package.version, package.import_root
        );
        let paths = metadata
            .effects
            .iter()
            .map(|item| &item.path)
            .chain(metadata.actions.iter().map(|item| &item.path))
            .chain(metadata.effect_summaries.iter().map(|item| &item.item))
            .chain(metadata.trace_specs.iter().map(|item| &item.path))
            .chain(
                metadata
                    .trace_spec_summaries
                    .iter()
                    .map(|item| &item.trace_spec),
            );
        for path in paths {
            if !seen.insert((metadata.package, path.clone())) {
                continue;
            }
            let Some(span) = anchors.item_span(metadata.package, path) else {
                continue;
            };
            output.push(etas_effects::ExternalArtifactAnchor {
                package: metadata.package.0,
                package_label: package_identity.clone(),
                item: path.clone(),
                span,
            });
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use etas_core::{SourceId, TextSize};

    use super::*;

    #[test]
    fn external_import_anchors_keep_package_identity_for_equal_item_paths() {
        let first_package = ExternalPackageId(1);
        let second_package = ExternalPackageId(2);
        let first_span = Span::empty(SourceId(10), TextSize(1));
        let second_span = Span::empty(SourceId(20), TextSize(2));
        let item = vec!["shared".to_owned(), "api".to_owned(), "run".to_owned()];
        let mut index = ExternalImportAnchorIndex::default();
        index.insert_item(first_package, item.clone(), first_span);
        index.insert_item(second_package, item.clone(), second_span);

        assert_eq!(index.item_span(first_package, &item), Some(first_span));
        assert_eq!(index.item_span(second_package, &item), Some(second_span));
        assert_eq!(index.item_span(ExternalPackageId(3), &item), None);
    }
}
