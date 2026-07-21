use std::collections::{HashMap, HashSet};

use etas_cache::{
    ArtifactFingerprint, ArtifactKey, ArtifactMeta, CachedArtifact, InvalidationReport,
    MemoryArtifactStore, ProjectRevision,
};
use etas_core::SourceId;
use etas_utils::PassControl;

use crate::artifact::{
    ArtifactDependencySummary, ArtifactReuseStatsSummary, BodyArtifactIdentityRecord,
    BodyArtifactReuseIndex, BodyArtifactReuseIndexRecord, BodyArtifactReuseKind, DiskCacheAccess,
    FRONTEND_ARTIFACT_SCHEMA_VERSION, FRONTEND_COMPILER_VERSION, FrontendArtifactDependency,
    FrontendArtifactKey, FrontendArtifactKind, FrontendArtifactManifest, FrontendArtifactRecord,
    FrontendUnitKey, ModuleImportExportSummary, PersistedFrontendArtifactDependency,
    SourceFingerprintSummary, cache_dependencies, fingerprint_text, frontend_artifact_meta,
    frontend_std_version,
};
use crate::incremental::{
    BodyArtifactIdentity, BodyArtifactReuseInput, CacheReuseReport, CachedEffectBodyArtifact,
    CachedTypeBodyArtifact, CheckRequest, CheckResponse, CheckScope, DiagnosticSet, SourceVersion,
    artifact_meta_matches,
};
use crate::pipeline::{FrontendPipelineRun, run_check_pipeline};
use crate::{BODY_UNIT_KIND, ParsedSource, ProjectInput, ProjectOutput, SourceSet, UnitKind};

use super::snapshot::diagnostic_sources;
use super::source_state::source_set_from_input;
use super::store::FrontendArtifactStore;
use super::{FrontendSessionError, ProjectSemanticDelta, ProjectSemanticSnapshot, cache_error};

pub(crate) struct FrontendProjectState<S = MemoryArtifactStore>
where
    S: FrontendArtifactStore,
{
    pub(super) revision: ProjectRevision,
    pub(super) input: ProjectInput,
    pub(super) sources: SourceSet,
    pub(super) manifest: FrontendArtifactManifest,
    pub(super) last_good_snapshot: Option<ProjectSemanticSnapshot>,
    pub(super) source_versions: HashMap<SourceId, SourceVersion>,
    pub(super) store: S,
    pub(super) pending_changed_sources: Vec<SourceId>,
    pub(super) pending_project_wide_change: bool,
    pub(super) pending_invalidation: InvalidationReport,
    pub(super) pending_body_artifact_reuse: BodyArtifactReuseInput,
    pub(super) std_version: String,
    pub(super) options_hash: String,
}

struct MetadataSummaryArtifact<T> {
    key: FrontendArtifactKey,
    fingerprint: ArtifactFingerprint,
    dependencies: Vec<FrontendArtifactDependency>,
    value: T,
}

impl<S> FrontendProjectState<S>
where
    S: FrontendArtifactStore,
{
    pub(super) fn new(input: ProjectInput, store: S) -> Self {
        let sources = source_set_from_input(&input);
        let options_hash = input.options.canonical_options_fingerprint();
        let source_versions = input
            .sources
            .iter()
            .map(|source| (source.id, SourceVersion::default()))
            .collect();
        Self {
            revision: ProjectRevision(0),
            input,
            sources,
            manifest: FrontendArtifactManifest::new(),
            last_good_snapshot: None,
            source_versions,
            store,
            pending_changed_sources: Vec::new(),
            pending_project_wide_change: false,
            pending_invalidation: InvalidationReport::default(),
            pending_body_artifact_reuse: BodyArtifactReuseInput::default(),
            std_version: frontend_std_version(),
            options_hash,
        }
    }

    pub(super) fn check(
        &mut self,
        request: CheckRequest,
    ) -> Result<CheckResponse, FrontendSessionError> {
        self.check_with_pipeline(request, run_check_pipeline)
    }

    fn check_with_pipeline(
        &mut self,
        request: CheckRequest,
        run_pipeline: impl FnOnce(
            ProjectInput,
            bool,
            Vec<SourceId>,
            bool,
            BodyArtifactReuseInput,
            HashMap<SourceId, ParsedSource>,
            CheckScope,
            bool,
        ) -> FrontendPipelineRun,
    ) -> Result<CheckResponse, FrontendSessionError> {
        let incremental = request.incremental_enabled();
        let disk_artifact_access = request.disk_artifact_access;
        let disk_read_enabled = disk_artifact_access.can_read();
        let memory_reuse_enabled = request.memory_artifact_reuse_enabled();

        if incremental && disk_read_enabled {
            self.restore_cached_artifact_manifest(disk_artifact_access)?;
        }
        let project_wide_change = if incremental {
            std::mem::take(&mut self.pending_project_wide_change)
        } else {
            self.pending_project_wide_change = false;
            false
        };
        let body_artifact_reuse = if incremental && memory_reuse_enabled && !project_wide_change {
            let pending = std::mem::take(&mut self.pending_body_artifact_reuse);
            if pending.is_empty() {
                self.cached_body_artifacts(disk_artifact_access)?
            } else {
                pending
            }
        } else {
            self.pending_body_artifact_reuse = BodyArtifactReuseInput::default();
            BodyArtifactReuseInput::default()
        };
        let parsed_source_reuse = if incremental && memory_reuse_enabled && !project_wide_change {
            self.cached_parsed_sources(&self.pending_changed_sources, disk_artifact_access)?
        } else {
            HashMap::new()
        };
        let pipeline_run = run_pipeline(
            self.input.clone(),
            incremental,
            self.pending_changed_sources.clone(),
            project_wide_change,
            body_artifact_reuse,
            parsed_source_reuse,
            request.scope,
            request.collect_pipeline_timing,
        );
        if let PassControl::Failed(failure) = &pipeline_run.control {
            return Err(FrontendSessionError::Pipeline(failure.message.clone()));
        }
        let output = pipeline_run.output;
        let diagnostics = DiagnosticSet {
            diagnostics: output.diagnostics.clone(),
        };
        let changed_sources = std::mem::take(&mut self.pending_changed_sources);
        let invalidation = std::mem::take(&mut self.pending_invalidation);
        let snapshot = ProjectSemanticSnapshot::from_output(self.revision, &output);

        self.manifest = FrontendArtifactManifest::from_output(self.revision, &output);
        let mut cache = self.store_project_artifacts(
            &output,
            memory_reuse_enabled && incremental && !project_wide_change,
            disk_artifact_access,
        )?;
        cache.record_reused_many(pipeline_run.reused_artifacts);
        let delta = Some(ProjectSemanticDelta::from_check(
            changed_sources.clone(),
            &output,
            &invalidation,
            &cache,
        ));
        if let Some(snapshot) = snapshot.clone() {
            self.last_good_snapshot = Some(snapshot);
        }
        let response_snapshot = request
            .snapshot_detail
            .includes_snapshot()
            .then(|| snapshot.clone())
            .flatten();

        Ok(CheckResponse {
            revision: self.revision,
            diagnostics,
            snapshot: response_snapshot,
            delta,
            cache,
            output,
            pipeline_records: pipeline_run.records,
        })
    }

    pub(super) fn cached_body_artifacts(
        &self,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<BodyArtifactReuseInput, FrontendSessionError> {
        let Some(snapshot) = self.last_good_snapshot.as_ref() else {
            return self.cached_body_artifacts_from_index(disk_artifact_access);
        };
        let mut type_outputs = Vec::new();
        let mut effect_outputs = Vec::new();
        for record in self.manifest.records.iter().filter(|record| {
            matches!(
                record.key.kind,
                FrontendArtifactKind::TypeFacts | FrontendArtifactKind::EffectFacts
            )
        }) {
            let FrontendUnitKey::Unit(unit) = &record.key.unit else {
                continue;
            };
            if unit.kind != BODY_UNIT_KIND {
                continue;
            }
            let unit_id = etas_frontend_unit_id(unit.id);
            let Some(identity) =
                BodyArtifactIdentity::for_unit(unit_id, &snapshot.units, &snapshot.sources)
            else {
                continue;
            };
            let cache_key = record.key.to_cache_key();
            match record.key.kind {
                FrontendArtifactKind::TypeFacts => {
                    let expected = self.meta_for_record(record);
                    if !self.artifact_is_reusable(&cache_key, &expected, disk_artifact_access)? {
                        continue;
                    }
                    let Some(artifact) = self
                        .store
                        .get_with_disk_access::<etas_types::TypeOutput>(
                            &cache_key,
                            disk_artifact_access,
                        )
                        .map_err(cache_error)?
                    else {
                        continue;
                    };
                    type_outputs.push(CachedTypeBodyArtifact {
                        unit: unit_id,
                        cache_key,
                        identity,
                        dependencies: record.dependencies.clone(),
                        output: artifact.value,
                    });
                }
                FrontendArtifactKind::EffectFacts => {
                    let expected = self.meta_for_record(record);
                    if !self.artifact_is_reusable(&cache_key, &expected, disk_artifact_access)? {
                        continue;
                    }
                    let Some(_) = self
                        .store
                        .get_with_disk_access::<etas_effects::EffectOutput>(
                            &cache_key,
                            disk_artifact_access,
                        )
                        .map_err(cache_error)?
                    else {
                        continue;
                    };
                    effect_outputs.push(CachedEffectBodyArtifact {
                        unit: unit_id,
                        cache_key,
                        identity,
                        dependencies: record.dependencies.clone(),
                    });
                }
                _ => {}
            }
        }
        Ok(BodyArtifactReuseInput {
            type_outputs,
            effect_outputs,
        })
    }

    fn cached_body_artifacts_from_index(
        &self,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<BodyArtifactReuseInput, FrontendSessionError> {
        let Some(index) = self.load_body_artifact_reuse_index(disk_artifact_access)? else {
            return Ok(BodyArtifactReuseInput::default());
        };
        let mut type_outputs = Vec::new();
        let mut effect_outputs = Vec::new();
        for record in &index.records {
            let Some(dependencies) = record.frontend_dependencies() else {
                return Err(FrontendSessionError::Cache(
                    "persisted body artifact reuse index contains an unsupported unit key"
                        .to_owned(),
                ));
            };
            let cache_key = record.artifact_key();
            let expected = self.meta_for_body_index_record(record, &dependencies);
            if !self.artifact_is_reusable(&cache_key, &expected, disk_artifact_access)? {
                continue;
            }
            let identity = BodyArtifactIdentity {
                source: SourceId(record.identity.source),
                item_index: record.identity.item_index,
                item_kind: record.identity.item_kind,
                text: record.identity.text.clone(),
            };
            let unit = etas_frontend_unit_id(u64::from(record.unit));
            match record.kind {
                BodyArtifactReuseKind::TypeFacts => {
                    let Some(artifact) = self
                        .store
                        .get_with_disk_access::<etas_types::TypeOutput>(
                            &cache_key,
                            disk_artifact_access,
                        )
                        .map_err(cache_error)?
                    else {
                        continue;
                    };
                    type_outputs.push(CachedTypeBodyArtifact {
                        unit,
                        cache_key,
                        identity,
                        dependencies,
                        output: artifact.value,
                    });
                }
                BodyArtifactReuseKind::EffectFacts => {
                    let Some(_) = self
                        .store
                        .get_with_disk_access::<etas_effects::EffectOutput>(
                            &cache_key,
                            disk_artifact_access,
                        )
                        .map_err(cache_error)?
                    else {
                        continue;
                    };
                    effect_outputs.push(CachedEffectBodyArtifact {
                        unit,
                        cache_key,
                        identity,
                        dependencies,
                    });
                }
            }
        }
        Ok(BodyArtifactReuseInput {
            type_outputs,
            effect_outputs,
        })
    }

    fn cached_parsed_sources(
        &self,
        changed_sources: &[SourceId],
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<HashMap<SourceId, ParsedSource>, FrontendSessionError> {
        let changed = changed_sources.iter().copied().collect::<HashSet<_>>();
        let mut parsed_sources = HashMap::new();
        for source in &self.input.sources {
            if changed.contains(&source.id) {
                continue;
            }
            let key = FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, source.id);
            let Some(record) = self.manifest.get(&key) else {
                continue;
            };
            let cache_key = key.to_cache_key();
            let expected = self.meta_for_record(record);
            if !self.artifact_is_reusable(&cache_key, &expected, disk_artifact_access)? {
                continue;
            }
            let Some(artifact) = self
                .store
                .get_with_disk_access::<ParsedSource>(&cache_key, disk_artifact_access)
                .map_err(cache_error)?
            else {
                continue;
            };
            parsed_sources.insert(source.id, artifact.value);
        }
        Ok(parsed_sources)
    }

    fn store_project_artifacts(
        &mut self,
        output: &ProjectOutput,
        reuse_enabled: bool,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<CacheReuseReport, FrontendSessionError> {
        let mut cache = CacheReuseReport::default();
        self.put_artifact(
            FrontendArtifactKey::project(FrontendArtifactKind::ParsedSourceSet),
            output.parsed_sources.clone(),
            reuse_enabled,
            disk_artifact_access,
            &mut cache,
        )?;
        if let Some(sources) = output.sources.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::SourceSet),
                sources,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(modules) = output.modules.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex),
                modules,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(units) = output.units.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::UnitTree),
                units,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(import_graph) = output.import_graph.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph),
                import_graph,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(module_topo_order) = output.module_topo_order.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ModuleTopoOrder),
                module_topo_order,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(affected_modules) = output.affected_modules.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::AffectedModuleSet),
                affected_modules,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(hir) = output.hir.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::HirProgram),
                hir,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(resolved_imports) = output.resolved_imports.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ResolvedImports),
                resolved_imports,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(resolved_paths) = output.resolved_paths.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ResolvedPaths),
                resolved_paths,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(signature_types) = output.signature_types.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::SignatureFacts),
                signature_types,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(signature_types) = output.signature_types.as_ref() {
            let mut items = signature_types
                .facts
                .item_signatures
                .keys()
                .copied()
                .collect::<Vec<_>>();
            items.sort_by_key(|item| item.0);
            for item in items {
                let Some(signature) = signature_types.facts.item_signatures.get(&item).cloned()
                else {
                    continue;
                };
                self.put_artifact(
                    FrontendArtifactKey::item(FrontendArtifactKind::SignatureFacts, item),
                    signature,
                    reuse_enabled,
                    disk_artifact_access,
                    &mut cache,
                )?;
            }
        }
        if let Some(top_level_lets) = output.top_level_lets.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::TopLevelLetFacts),
                top_level_lets,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let (Some(units), Some(sources)) = (&output.units, &output.sources) {
            let mut body_units = units
                .nodes
                .iter()
                .filter_map(|(unit, node)| (node.kind == UnitKind::Body).then_some(unit))
                .collect::<Vec<_>>();
            body_units.sort_by_key(|unit| unit.0);
            for unit in body_units {
                let Some(identity) = BodyArtifactIdentity::for_unit(unit, units, sources) else {
                    continue;
                };
                self.put_artifact(
                    FrontendArtifactKey::unit(
                        FrontendArtifactKind::HirBody,
                        etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
                    ),
                    identity,
                    reuse_enabled,
                    disk_artifact_access,
                    &mut cache,
                )?;
            }
        }
        let mut type_body_units = output.type_body_outputs.keys().copied().collect::<Vec<_>>();
        type_body_units.sort_by_key(|unit| unit.0);
        for unit in type_body_units {
            let Some(types) = output.type_body_outputs.get(&unit).cloned() else {
                continue;
            };
            self.put_artifact(
                FrontendArtifactKey::unit(
                    FrontendArtifactKind::TypeFacts,
                    etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
                ),
                types,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(types) = output.types.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::TypeFacts),
                types,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        let mut effect_body_units = output
            .effect_body_outputs
            .keys()
            .copied()
            .collect::<Vec<_>>();
        effect_body_units.sort_by_key(|unit| unit.0);
        for unit in effect_body_units {
            let Some(effects) = output.effect_body_outputs.get(&unit).cloned() else {
                continue;
            };
            self.put_artifact(
                FrontendArtifactKey::unit(
                    FrontendArtifactKind::EffectFacts,
                    etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
                ),
                effects,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(artifacts) = output.effect_pipeline_artifacts.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::EffectPipelineArtifacts),
                artifacts,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(effects) = output.effects.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::EffectFacts),
                effects,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(entry) = output.entry.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ProjectEntry),
                entry,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(reachability) = output.reachability.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::ReachabilityFacts),
                reachability,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        if let Some(checked) = output.checked.clone() {
            self.put_artifact(
                FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject),
                checked,
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        for parsed in &output.parsed_sources {
            self.put_artifact(
                FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, parsed.source),
                parsed.clone(),
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        for source in diagnostic_sources(output) {
            self.put_artifact(
                FrontendArtifactKey::source(FrontendArtifactKind::Diagnostics, source),
                DiagnosticSet {
                    diagnostics: output
                        .diagnostics
                        .iter()
                        .filter(|diagnostic| diagnostic.primary.span.source == source)
                        .cloned()
                        .collect(),
                },
                reuse_enabled,
                disk_artifact_access,
                &mut cache,
            )?;
        }
        self.put_body_artifact_reuse_index(
            output,
            reuse_enabled,
            disk_artifact_access,
            &mut cache,
        )?;
        self.put_metadata_summaries(output, reuse_enabled, disk_artifact_access, &mut cache)?;
        self.put_frontend_artifact_manifest(reuse_enabled, disk_artifact_access, &mut cache)?;
        Ok(cache)
    }

    fn put_artifact<T: Clone + 'static>(
        &mut self,
        key: FrontendArtifactKey,
        value: T,
        reuse_enabled: bool,
        disk_artifact_access: DiskCacheAccess,
        cache: &mut CacheReuseReport,
    ) -> Result<(), FrontendSessionError> {
        let cache_key = key.to_cache_key();
        let fingerprint = self
            .manifest
            .get(&key)
            .map(|record| record.fingerprint)
            .unwrap_or_else(|| fingerprint_text(&[cache_key.to_string().as_str()]));
        let meta = self
            .manifest
            .get(&key)
            .map(|record| self.meta_for_record(record))
            .unwrap_or_else(|| {
                ArtifactMeta::new(
                    self.revision,
                    fingerprint,
                    FRONTEND_COMPILER_VERSION,
                    FRONTEND_ARTIFACT_SCHEMA_VERSION,
                )
                .with_std_version(&self.std_version)
                .with_options_hash(&self.options_hash)
            });
        if reuse_enabled && self.artifact_is_reusable(&cache_key, &meta, disk_artifact_access)? {
            cache.record_reused(cache_key);
            return Ok(());
        }
        self.store
            .put_with_disk_access(
                CachedArtifact {
                    key: cache_key.clone(),
                    meta,
                    value,
                },
                disk_artifact_access,
            )
            .map_err(cache_error)?;
        cache.record_stored(cache_key);
        Ok(())
    }

    fn put_metadata_summaries(
        &mut self,
        output: &ProjectOutput,
        reuse_enabled: bool,
        disk_artifact_access: DiskCacheAccess,
        cache: &mut CacheReuseReport,
    ) -> Result<(), FrontendSessionError> {
        if let Some(sources) = output.sources.as_ref() {
            let summary = SourceFingerprintSummary::from_sources(sources);
            let dependencies = self.summary_dependencies([FrontendArtifactKey::project(
                FrontendArtifactKind::SourceSet,
            )]);
            self.put_metadata_summary(
                MetadataSummaryArtifact {
                    key: FrontendArtifactKey::project(
                        FrontendArtifactKind::SourceFingerprintSummary,
                    ),
                    fingerprint: summary.fingerprint(),
                    dependencies,
                    value: summary,
                },
                reuse_enabled,
                disk_artifact_access,
                cache,
            )?;
        }

        if let Some(modules) = output.modules.as_ref() {
            let summary =
                ModuleImportExportSummary::from_project(modules, output.import_graph.as_ref());
            let dependencies = self.summary_dependencies([
                FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex),
                FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph),
            ]);
            self.put_metadata_summary(
                MetadataSummaryArtifact {
                    key: FrontendArtifactKey::project(
                        FrontendArtifactKind::ModuleImportExportSummary,
                    ),
                    fingerprint: summary.fingerprint(),
                    dependencies,
                    value: summary,
                },
                reuse_enabled,
                disk_artifact_access,
                cache,
            )?;
        }

        if !self.manifest.is_empty() {
            let summary = ArtifactDependencySummary::from_manifest(&self.manifest);
            let dependencies = self
                .manifest
                .cache_keys()
                .filter_map(|key| FrontendArtifactKey::from_cache_key(&key))
                .filter_map(|key| self.summary_dependency(key))
                .collect::<Vec<_>>();
            self.put_metadata_summary(
                MetadataSummaryArtifact {
                    key: FrontendArtifactKey::project(
                        FrontendArtifactKind::ArtifactDependencySummary,
                    ),
                    fingerprint: summary.fingerprint(),
                    dependencies,
                    value: summary,
                },
                reuse_enabled,
                disk_artifact_access,
                cache,
            )?;
        }

        let reuse_summary = ArtifactReuseStatsSummary::from_keys(
            cache.reused_artifacts.clone(),
            cache.stored_artifacts.clone(),
        );
        let dependencies = cache
            .reused_artifacts
            .iter()
            .chain(cache.stored_artifacts.iter())
            .filter_map(FrontendArtifactKey::from_cache_key)
            .filter_map(|key| self.summary_dependency(key))
            .collect::<Vec<_>>();
        self.put_metadata_summary(
            MetadataSummaryArtifact {
                key: FrontendArtifactKey::project(FrontendArtifactKind::ArtifactReuseStats),
                fingerprint: reuse_summary.fingerprint(),
                dependencies,
                value: reuse_summary,
            },
            reuse_enabled,
            disk_artifact_access,
            cache,
        )?;
        Ok(())
    }

    fn put_metadata_summary<T: Clone + 'static>(
        &mut self,
        artifact: MetadataSummaryArtifact<T>,
        reuse_enabled: bool,
        disk_artifact_access: DiskCacheAccess,
        cache: &mut CacheReuseReport,
    ) -> Result<(), FrontendSessionError> {
        let record = FrontendArtifactRecord {
            key: artifact.key.clone(),
            revision: self.revision,
            fingerprint: artifact.fingerprint,
            dependencies: artifact.dependencies,
            diagnostics: Vec::new(),
        };
        self.manifest.record(record.clone());
        let cache_key = artifact.key.to_cache_key();
        let meta = self.meta_for_record(&record);
        if reuse_enabled && self.artifact_is_reusable(&cache_key, &meta, disk_artifact_access)? {
            cache.record_reused(cache_key);
            return Ok(());
        }
        self.store
            .put_with_disk_access(
                CachedArtifact {
                    key: cache_key.clone(),
                    meta,
                    value: artifact.value,
                },
                disk_artifact_access,
            )
            .map_err(cache_error)?;
        cache.record_stored(cache_key);
        Ok(())
    }

    fn summary_dependencies(
        &self,
        keys: impl IntoIterator<Item = FrontendArtifactKey>,
    ) -> Vec<FrontendArtifactDependency> {
        keys.into_iter()
            .filter_map(|key| self.summary_dependency(key))
            .collect()
    }

    fn summary_dependency(&self, key: FrontendArtifactKey) -> Option<FrontendArtifactDependency> {
        self.manifest
            .get(&key)
            .map(|record| FrontendArtifactDependency {
                key,
                fingerprint: Some(record.fingerprint),
            })
    }

    fn meta_for_record(&self, record: &FrontendArtifactRecord) -> ArtifactMeta {
        frontend_artifact_meta(
            self.revision,
            record.fingerprint,
            &self.std_version,
            &self.options_hash,
        )
        .with_dependencies(cache_dependencies(&record.dependencies))
    }

    fn meta_for_body_index_record(
        &self,
        record: &BodyArtifactReuseIndexRecord,
        dependencies: &[crate::artifact::FrontendArtifactDependency],
    ) -> ArtifactMeta {
        frontend_artifact_meta(
            record.project_revision(),
            record.artifact_fingerprint(),
            &self.std_version,
            &self.options_hash,
        )
        .with_dependencies(cache_dependencies(dependencies))
    }

    fn artifact_is_reusable(
        &self,
        key: &ArtifactKey,
        expected: &ArtifactMeta,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<bool, FrontendSessionError> {
        let Some(stored) = self
            .store
            .meta_with_disk_access(key, disk_artifact_access)
            .map_err(cache_error)?
        else {
            return Ok(false);
        };
        Ok(artifact_meta_matches(&stored, expected))
    }

    fn put_body_artifact_reuse_index(
        &mut self,
        output: &ProjectOutput,
        reuse_enabled: bool,
        disk_artifact_access: DiskCacheAccess,
        cache: &mut CacheReuseReport,
    ) -> Result<(), FrontendSessionError> {
        let index = self.body_artifact_reuse_index(output);
        if index.is_empty() {
            return Ok(());
        }
        let key = FrontendArtifactKey::project(FrontendArtifactKind::BodyArtifactReuseIndex)
            .to_cache_key();
        let meta = frontend_artifact_meta(
            self.revision,
            index.fingerprint(),
            &self.std_version,
            &self.options_hash,
        )
        .with_dependencies(index.dependencies());
        if reuse_enabled && self.artifact_is_reusable(&key, &meta, disk_artifact_access)? {
            cache.record_reused(key);
            return Ok(());
        }
        self.store
            .put_with_disk_access(
                CachedArtifact {
                    key: key.clone(),
                    meta,
                    value: index,
                },
                disk_artifact_access,
            )
            .map_err(cache_error)?;
        cache.record_stored(key);
        Ok(())
    }

    fn put_frontend_artifact_manifest(
        &mut self,
        reuse_enabled: bool,
        disk_artifact_access: DiskCacheAccess,
        cache: &mut CacheReuseReport,
    ) -> Result<(), FrontendSessionError> {
        if self.manifest.is_empty() {
            return Ok(());
        }
        let key =
            FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
        let meta = frontend_artifact_meta(
            self.revision,
            self.manifest.fingerprint(),
            &self.std_version,
            &self.options_hash,
        )
        .with_dependencies(manifest_dependency_keys(&self.manifest));
        if reuse_enabled && self.artifact_is_reusable(&key, &meta, disk_artifact_access)? {
            cache.record_reused(key);
            return Ok(());
        }
        self.store
            .put_with_disk_access(
                CachedArtifact {
                    key: key.clone(),
                    meta,
                    value: self.manifest.clone(),
                },
                disk_artifact_access,
            )
            .map_err(cache_error)?;
        cache.record_stored(key);
        Ok(())
    }

    pub(super) fn restore_cached_artifact_manifest(
        &mut self,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<(), FrontendSessionError> {
        if !self.manifest.is_empty() {
            return Ok(());
        }
        let Some(manifest) = self.load_frontend_artifact_manifest(disk_artifact_access)? else {
            return Ok(());
        };
        self.manifest = manifest;
        Ok(())
    }

    fn load_frontend_artifact_manifest(
        &self,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<Option<FrontendArtifactManifest>, FrontendSessionError> {
        let key =
            FrontendArtifactKey::project(FrontendArtifactKind::ArtifactManifest).to_cache_key();
        let Some(meta) = self
            .store
            .meta_with_disk_access(&key, disk_artifact_access)
            .map_err(cache_error)?
        else {
            return Ok(None);
        };
        if meta.compiler_version != FRONTEND_COMPILER_VERSION
            || meta.cache_schema_version != FRONTEND_ARTIFACT_SCHEMA_VERSION
            || meta.std_version.as_deref() != Some(self.std_version.as_str())
            || meta.options_hash.as_deref() != Some(self.options_hash.as_str())
        {
            return Ok(None);
        }
        let Some(artifact) = self
            .store
            .get_with_disk_access::<FrontendArtifactManifest>(&key, disk_artifact_access)
            .map_err(cache_error)?
        else {
            return Err(FrontendSessionError::Cache(format!(
                "artifact manifest metadata exists but payload is missing for {key}"
            )));
        };
        let expected = frontend_artifact_meta(
            artifact.meta.revision,
            artifact.value.fingerprint(),
            &self.std_version,
            &self.options_hash,
        )
        .with_dependencies(manifest_dependency_keys(&artifact.value));
        if !artifact_meta_matches(&artifact.meta, &expected) {
            return Ok(None);
        }
        Ok(Some(artifact.value))
    }

    fn load_body_artifact_reuse_index(
        &self,
        disk_artifact_access: DiskCacheAccess,
    ) -> Result<Option<BodyArtifactReuseIndex>, FrontendSessionError> {
        let key = FrontendArtifactKey::project(FrontendArtifactKind::BodyArtifactReuseIndex)
            .to_cache_key();
        let Some(meta) = self
            .store
            .meta_with_disk_access(&key, disk_artifact_access)
            .map_err(cache_error)?
        else {
            return Ok(None);
        };
        if meta.compiler_version != FRONTEND_COMPILER_VERSION
            || meta.cache_schema_version != FRONTEND_ARTIFACT_SCHEMA_VERSION
            || meta.std_version.as_deref() != Some(self.std_version.as_str())
            || meta.options_hash.as_deref() != Some(self.options_hash.as_str())
        {
            return Ok(None);
        }
        let Some(artifact) = self
            .store
            .get_with_disk_access::<BodyArtifactReuseIndex>(&key, disk_artifact_access)
            .map_err(cache_error)?
        else {
            return Ok(None);
        };
        let expected = frontend_artifact_meta(
            self.revision,
            artifact.value.fingerprint(),
            &self.std_version,
            &self.options_hash,
        )
        .with_dependencies(artifact.value.dependencies());
        if !crate::incremental::artifact_meta_matches(&artifact.meta, &expected) {
            return Ok(None);
        }
        Ok(Some(artifact.value))
    }

    fn body_artifact_reuse_index(&self, output: &ProjectOutput) -> BodyArtifactReuseIndex {
        let Some(units) = output.units.as_ref() else {
            return BodyArtifactReuseIndex::default();
        };
        let Some(sources) = output.sources.as_ref() else {
            return BodyArtifactReuseIndex::default();
        };
        let mut records = self
            .manifest
            .records
            .iter()
            .filter_map(|record| {
                let kind = match record.key.kind {
                    FrontendArtifactKind::TypeFacts => BodyArtifactReuseKind::TypeFacts,
                    FrontendArtifactKind::EffectFacts => BodyArtifactReuseKind::EffectFacts,
                    _ => return None,
                };
                let FrontendUnitKey::Unit(unit) = &record.key.unit else {
                    return None;
                };
                if unit.kind != BODY_UNIT_KIND {
                    return None;
                }
                let unit_id = etas_frontend_unit_id(unit.id);
                let identity = BodyArtifactIdentity::for_unit(unit_id, units, sources)?;
                Some(BodyArtifactReuseIndexRecord {
                    kind,
                    unit: unit_id.0,
                    revision: record.revision.0,
                    fingerprint: record.fingerprint.bytes(),
                    identity: BodyArtifactIdentityRecord {
                        source: identity.source.0,
                        item_index: identity.item_index,
                        item_kind: identity.item_kind,
                        text: identity.text,
                    },
                    dependencies: record
                        .dependencies
                        .iter()
                        .map(PersistedFrontendArtifactDependency::from_frontend)
                        .collect(),
                })
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| (record.unit, record.kind as u8));
        BodyArtifactReuseIndex { records }
    }
}

fn etas_frontend_unit_id(id: u64) -> crate::UnitId {
    crate::UnitId(id as u32)
}

fn manifest_dependency_keys(manifest: &FrontendArtifactManifest) -> Vec<ArtifactKey> {
    let mut keys = manifest.cache_keys().collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use etas_core::SourceId;
    use etas_utils::{PassControl, PassFailure};

    use crate::incremental::{CheckMode, SourceVersion};
    use crate::pipeline::FrontendPipelineRun;
    use crate::{
        ProjectChangeSet, ProjectInput, SourceChange, SourceInput, SourceKind,
        session::FrontendSessionError,
    };

    use super::FrontendProjectState;

    #[test]
    fn pipeline_failure_preserves_pending_changes_for_retry() {
        let source = SourceId(41);
        let mut state = FrontendProjectState::new(
            ProjectInput::single_source(SourceInput {
                id: source,
                path: None,
                text: "flow main() -> unit { return; }".to_owned(),
                kind: SourceKind::SingleFileInput,
            }),
            etas_cache::MemoryArtifactStore::new(),
        );
        state
            .apply_changes(ProjectChangeSet {
                revision: etas_cache::ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source,
                    version: SourceVersion(1),
                    text: "flow main() -> unit { let value = 1; return; }".to_owned(),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            })
            .expect("source replacement should be accepted");

        let error = state
            .check_with_pipeline(
                crate::incremental::CheckRequest {
                    mode: CheckMode::Incremental,
                    ..Default::default()
                },
                |input,
                 incremental,
                 changed_sources,
                 project_wide_change,
                 _reuse,
                 parsed_reuse,
                 _scope,
                 collect_timing| {
                    assert_eq!(
                        input.sources[0].text,
                        "flow main() -> unit { let value = 1; return; }"
                    );
                    assert!(incremental);
                    assert_eq!(changed_sources, vec![source]);
                    assert!(!project_wide_change);
                    assert!(parsed_reuse.is_empty());
                    assert!(!collect_timing);
                    FrontendPipelineRun {
                        output: crate::ProjectContext::new(input).into_project_output(),
                        reused_artifacts: Vec::new(),
                        control: PassControl::Failed(PassFailure::new(
                            "synthetic pipeline failure",
                        )),
                        records: Vec::new(),
                    }
                },
            )
            .expect_err("pipeline failure should be surfaced");

        assert_eq!(
            error,
            FrontendSessionError::Pipeline("synthetic pipeline failure".to_owned())
        );
        assert_eq!(state.pending_changed_sources, vec![source]);
        assert!(!state.pending_project_wide_change);
    }
}
