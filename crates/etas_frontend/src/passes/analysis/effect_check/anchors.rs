use crate::ProjectEnvironmentInput;
pub(super) use crate::project::ExternalImportAnchorIndex;
use std::collections::{HashMap, HashSet};

pub(super) fn external_artifact_anchors(
    environment: &ProjectEnvironmentInput,
    anchors: &ExternalImportAnchorIndex,
) -> Vec<etas_effects::ExternalArtifactAnchor> {
    let packages = environment
        .external_packages
        .iter()
        .map(|package| (package.id, package))
        .collect::<HashMap<_, _>>();
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for metadata in &environment.external_public_metadata {
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
