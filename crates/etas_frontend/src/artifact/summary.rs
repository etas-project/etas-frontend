use std::collections::BTreeMap;

use etas_cache::{ArtifactFingerprint, ArtifactKey};
use etas_core::SourceId;

use super::{FrontendArtifactManifest, fingerprint_text};
use crate::{ImportGraph, ModuleIndex, SourceSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFingerprintSummary {
    pub sources: Vec<SourceFingerprintRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFingerprintRecord {
    pub source: SourceId,
    pub path: Option<String>,
    pub kind: String,
    pub text_hash: ArtifactFingerprint,
    pub byte_len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleImportExportSummary {
    pub modules: Vec<ModuleSurfaceRecord>,
    pub imports: Vec<ImportEdgeSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleSurfaceRecord {
    pub module: u32,
    pub path: String,
    pub parts: Vec<ModulePartSurfaceRecord>,
    pub public_export_count: usize,
    pub re_export_count: usize,
    pub surface_hash: ArtifactFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModulePartSurfaceRecord {
    pub part: u32,
    pub source: SourceId,
    pub import_count: usize,
    pub item_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEdgeSummary {
    pub from: u32,
    pub from_part: u32,
    pub candidate: String,
    pub resolved: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactDependencySummary {
    pub records: Vec<ArtifactDependencySummaryRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactDependencySummaryRecord {
    pub artifact: ArtifactKey,
    pub fingerprint: ArtifactFingerprint,
    pub dependencies: Vec<ArtifactDependencyEdgeSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactDependencyEdgeSummary {
    pub dependency: ArtifactKey,
    pub fingerprint: Option<ArtifactFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactReuseStatsSummary {
    pub reused_count: usize,
    pub stored_count: usize,
    pub reused_by_kind: Vec<ArtifactKindCount>,
    pub stored_by_kind: Vec<ArtifactKindCount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactKindCount {
    pub namespace: String,
    pub kind: String,
    pub count: usize,
}

impl SourceFingerprintSummary {
    pub fn from_sources(sources: &SourceSet) -> Self {
        let mut records = sources
            .files
            .iter()
            .map(|source| SourceFingerprintRecord {
                source: source.id,
                path: source.path.as_ref().map(|path| path.display().to_string()),
                kind: format!("{:?}", source.kind),
                text_hash: fingerprint_text(&[source.text.as_ref()]),
                byte_len: source.text.len() as u64,
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.source.0);
        Self { sources: records }
    }

    pub fn fingerprint(&self) -> ArtifactFingerprint {
        let mut parts = vec!["source_fingerprint_summary:v1".to_owned()];
        for record in &self.sources {
            parts.push(format!("source:{}", record.source.0));
            parts.push(format!("path:{:?}", record.path));
            parts.push(format!("kind:{}", record.kind));
            parts.push(format!("text_hash:{}", record.text_hash));
            parts.push(format!("byte_len:{}", record.byte_len));
        }
        fingerprint_owned_parts(&parts)
    }
}

impl ModuleImportExportSummary {
    pub fn from_project(modules: &ModuleIndex, import_graph: Option<&ImportGraph>) -> Self {
        let mut module_records = modules
            .modules
            .iter()
            .map(|(module, info)| {
                let mut parts = info
                    .parts
                    .iter()
                    .filter_map(|part| {
                        modules
                            .parts
                            .get(*part)
                            .map(|part_info| ModulePartSurfaceRecord {
                                part: part.0,
                                source: part_info.source,
                                import_count: part_info.imports.len(),
                                item_count: part_info.items.len(),
                            })
                    })
                    .collect::<Vec<_>>();
                parts.sort_by_key(|part| part.part);
                let mut exports = info
                    .visibility_exports
                    .items
                    .iter()
                    .map(|(name, item)| {
                        format!(
                            "{}:{:?}:{}:{}",
                            name, item.visibility, item.part.0, item.item.index
                        )
                    })
                    .collect::<Vec<_>>();
                exports.sort();
                let mut re_exports = info
                    .visibility_exports
                    .re_exports
                    .iter()
                    .map(|re_export| module_path_key(&re_export.target))
                    .collect::<Vec<_>>();
                re_exports.sort();
                let surface_hash = fingerprint_owned_parts(
                    &exports
                        .iter()
                        .chain(re_exports.iter())
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                ModuleSurfaceRecord {
                    module: module.0,
                    path: module_path_key(&info.path),
                    parts,
                    public_export_count: info.visibility_exports.items.len(),
                    re_export_count: info.visibility_exports.re_exports.len(),
                    surface_hash,
                }
            })
            .collect::<Vec<_>>();
        module_records.sort_by(|left, right| left.path.cmp(&right.path));

        let mut imports = import_graph
            .map(|graph| {
                graph
                    .edges
                    .iter()
                    .map(|edge| ImportEdgeSummary {
                        from: edge.from.0,
                        from_part: edge.from_part.0,
                        candidate: module_path_key(&edge.candidate),
                        resolved: edge.resolved.map(|module| module.0),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        imports.sort_by(|left, right| {
            (
                left.from,
                left.from_part,
                left.candidate.as_str(),
                left.resolved,
            )
                .cmp(&(
                    right.from,
                    right.from_part,
                    right.candidate.as_str(),
                    right.resolved,
                ))
        });

        Self {
            modules: module_records,
            imports,
        }
    }

    pub fn fingerprint(&self) -> ArtifactFingerprint {
        let mut parts = vec!["module_import_export_summary:v1".to_owned()];
        for module in &self.modules {
            parts.push(format!("module:{}", module.module));
            parts.push(format!("path:{}", module.path));
            parts.push(format!("exports:{}", module.public_export_count));
            parts.push(format!("re_exports:{}", module.re_export_count));
            parts.push(format!("surface_hash:{}", module.surface_hash));
            for part in &module.parts {
                parts.push(format!(
                    "part:{}:{}:{}:{}",
                    part.part, part.source.0, part.import_count, part.item_count
                ));
            }
        }
        for import in &self.imports {
            parts.push(format!(
                "import:{}:{}:{}:{:?}",
                import.from, import.from_part, import.candidate, import.resolved
            ));
        }
        fingerprint_owned_parts(&parts)
    }
}

impl ArtifactDependencySummary {
    pub fn from_manifest(manifest: &FrontendArtifactManifest) -> Self {
        let mut records = manifest
            .records
            .iter()
            .map(|record| {
                let mut dependencies = record
                    .dependencies
                    .iter()
                    .map(|dependency| ArtifactDependencyEdgeSummary {
                        dependency: dependency.key.to_cache_key(),
                        fingerprint: dependency.fingerprint,
                    })
                    .collect::<Vec<_>>();
                dependencies.sort_by_key(|dependency| dependency.dependency.to_string());
                ArtifactDependencySummaryRecord {
                    artifact: record.key.to_cache_key(),
                    fingerprint: record.fingerprint,
                    dependencies,
                }
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.artifact.to_string());
        Self { records }
    }

    pub fn fingerprint(&self) -> ArtifactFingerprint {
        let mut parts = vec!["artifact_dependency_summary:v1".to_owned()];
        for record in &self.records {
            parts.push(format!("artifact:{}", record.artifact));
            parts.push(format!("fingerprint:{}", record.fingerprint));
            for dependency in &record.dependencies {
                parts.push(format!("dependency:{}", dependency.dependency));
                parts.push(format!(
                    "dependency_fingerprint:{:?}",
                    dependency.fingerprint
                ));
            }
        }
        fingerprint_owned_parts(&parts)
    }
}

impl ArtifactReuseStatsSummary {
    pub fn from_keys(
        reused: impl IntoIterator<Item = ArtifactKey>,
        stored: impl IntoIterator<Item = ArtifactKey>,
    ) -> Self {
        let reused = reused.into_iter().collect::<Vec<_>>();
        let stored = stored.into_iter().collect::<Vec<_>>();
        Self {
            reused_count: reused.len(),
            stored_count: stored.len(),
            reused_by_kind: kind_counts(reused),
            stored_by_kind: kind_counts(stored),
        }
    }

    pub fn fingerprint(&self) -> ArtifactFingerprint {
        let mut parts = vec![
            "artifact_reuse_stats:v1".to_owned(),
            format!("reused_count:{}", self.reused_count),
            format!("stored_count:{}", self.stored_count),
        ];
        for count in &self.reused_by_kind {
            parts.push(format!(
                "reused:{}:{}:{}",
                count.namespace, count.kind, count.count
            ));
        }
        for count in &self.stored_by_kind {
            parts.push(format!(
                "stored:{}:{}:{}",
                count.namespace, count.kind, count.count
            ));
        }
        fingerprint_owned_parts(&parts)
    }
}

fn kind_counts(keys: Vec<ArtifactKey>) -> Vec<ArtifactKindCount> {
    let mut counts = BTreeMap::<(String, String), usize>::new();
    for key in keys {
        *counts
            .entry((
                key.namespace.as_str().to_owned(),
                key.kind.as_str().to_owned(),
            ))
            .or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((namespace, kind), count)| ArtifactKindCount {
            namespace,
            kind,
            count,
        })
        .collect()
}

fn module_path_key(path: &crate::ModulePath) -> String {
    path.segments.join(".")
}

fn fingerprint_owned_parts(parts: &[String]) -> ArtifactFingerprint {
    let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
    fingerprint_text(&refs)
}
