use std::collections::HashSet;

use etas_core::SourceId;

use crate::artifact::{DiskCacheAccess, changed_compiler_options_hash};
use crate::incremental::{
    BodyArtifactReuseInput, ChangeSummary, DependencyOverlayChange, DirtyArtifactRoots,
    EnvironmentChange, ProjectChangeSet, SourceChange, SourceVersion, TextEdit,
    invalidate_dirty_roots,
};
use crate::{SourceInput, SourceKind};

use super::source_state::source_file_from_input;
use super::store::FrontendArtifactStore;
use super::{FrontendSessionError, cache_error};
use crate::session::project::FrontendProjectState;

impl<S> FrontendProjectState<S>
where
    S: FrontendArtifactStore,
{
    pub(super) fn apply_changes(
        &mut self,
        changes: ProjectChangeSet,
    ) -> Result<ChangeSummary, FrontendSessionError> {
        if changes.revision <= self.revision {
            return Err(FrontendSessionError::NonIncreasingProjectRevision {
                current: self.revision,
                requested: changes.revision,
            });
        }

        let mut added_sources = Vec::new();
        let mut removed_sources = Vec::new();
        let mut changed_sources = Vec::new();
        let mut source_membership_change = false;

        let option_changes = changes.option_changes;
        let mut broad_project_change =
            changes.environment_change.is_some() || !option_changes.is_empty();

        if !option_changes.is_empty() {
            let changed_options = option_changes
                .iter()
                .map(|change| change.name.clone())
                .collect::<Vec<_>>();
            self.options_hash = changed_compiler_options_hash(
                &self.options_hash,
                changes.revision,
                &changed_options,
            );
        }

        if let Some(change) = changes.environment_change {
            match change {
                EnvironmentChange::Replace(environment) => {
                    self.input.environment = environment;
                }
            }
        }

        for change in changes.source_changes {
            match change {
                SourceChange::Add(source) => {
                    if self
                        .input
                        .sources
                        .iter()
                        .any(|existing| existing.id == source.id)
                    {
                        return Err(FrontendSessionError::DuplicateSource(source.id));
                    }
                    source_membership_change = true;
                    added_sources.push(source.id);
                    changed_sources.push(source.id);
                    self.source_versions
                        .insert(source.id, SourceVersion::default());
                    self.sources.files.push(source_file_from_input(&source));
                    self.input.sources.push(source);
                }
                SourceChange::Remove(source) => {
                    source_membership_change = true;
                    let index = self.source_index(source)?;
                    self.input.sources.remove(index);
                    let file_index = self.source_file_index(source)?;
                    self.sources.files.remove(file_index);
                    self.source_versions.remove(&source);
                    removed_sources.push(source);
                    changed_sources.push(source);
                }
                SourceChange::Replace {
                    source,
                    version,
                    text,
                } => {
                    self.validate_source_version(source, version)?;
                    let index = self.source_index(source)?;
                    self.input.sources[index].text = text;
                    self.replace_source_file(self.input.sources[index].clone())?;
                    self.source_versions.insert(source, version);
                    changed_sources.push(source);
                }
                SourceChange::Edit {
                    source,
                    version,
                    edits,
                } => {
                    self.validate_source_version(source, version)?;
                    let index = self.source_index(source)?;
                    apply_text_edits(source, &mut self.input.sources[index].text, edits)?;
                    self.replace_source_file(self.input.sources[index].clone())?;
                    self.source_versions.insert(source, version);
                    changed_sources.push(source);
                }
            }
        }

        for change in changes.dependency_overlay_changes {
            self.apply_dependency_overlay_change(change, &mut added_sources, &mut changed_sources)?;
        }

        changed_sources.sort_by_key(|source| source.0);
        changed_sources.dedup();
        broad_project_change |= source_membership_change;

        self.restore_cached_artifact_manifest(DiskCacheAccess::disabled())?;
        self.pending_body_artifact_reuse = if broad_project_change {
            BodyArtifactReuseInput::default()
        } else {
            self.cached_body_artifacts(DiskCacheAccess::disabled())?
        };
        let dirty_roots = DirtyArtifactRoots::from_sources(&changed_sources, broad_project_change);
        let invalidation =
            invalidate_dirty_roots(&mut self.store, dirty_roots).map_err(cache_error)?;
        self.pending_changed_sources = changed_sources.clone();
        self.pending_project_wide_change = broad_project_change;
        self.pending_invalidation = invalidation.clone();
        self.revision = changes.revision;

        Ok(ChangeSummary {
            revision: self.revision,
            changed_sources,
            added_sources,
            removed_sources,
            invalidated_artifacts: invalidation.invalidated,
        })
    }

    fn apply_dependency_overlay_change(
        &mut self,
        change: DependencyOverlayChange,
        added_sources: &mut Vec<SourceId>,
        changed_sources: &mut Vec<SourceId>,
    ) -> Result<(), FrontendSessionError> {
        if change.added_sources.is_empty() {
            return Err(FrontendSessionError::EmptyDependencyOverlay {
                package: change.package,
                import_root: change.import_root,
            });
        }

        let mut batch_ids = HashSet::new();
        let mut batch_paths = HashSet::new();
        for source in &change.added_sources {
            match &source.kind {
                SourceKind::DependencySourceOverlay {
                    package,
                    import_root,
                } if *package == change.package && import_root == &change.import_root => {}
                SourceKind::DependencySourceOverlay { .. } => {
                    return Err(FrontendSessionError::MismatchedDependencyOverlaySource {
                        source: source.id,
                        expected_package: change.package,
                        expected_import_root: change.import_root.clone(),
                    });
                }
                _ => {
                    return Err(FrontendSessionError::InvalidDependencyOverlaySource {
                        source: source.id,
                    });
                }
            }

            if !batch_ids.insert(source.id)
                || self
                    .input
                    .sources
                    .iter()
                    .any(|existing| existing.id == source.id)
            {
                return Err(FrontendSessionError::DuplicateSource(source.id));
            }
            if let Some(path) = &source.path {
                if !batch_paths.insert(path.clone())
                    || self
                        .input
                        .sources
                        .iter()
                        .any(|existing| existing.path.as_ref() == Some(path))
                {
                    return Err(FrontendSessionError::DuplicateSourcePath(path.clone()));
                }
            }
        }

        for source in change.added_sources {
            added_sources.push(source.id);
            changed_sources.push(source.id);
            self.source_versions
                .insert(source.id, SourceVersion::default());
            self.sources.files.push(source_file_from_input(&source));
            self.input.sources.push(source);
        }
        Ok(())
    }

    fn source_index(&self, source: SourceId) -> Result<usize, FrontendSessionError> {
        self.input
            .sources
            .iter()
            .position(|existing| existing.id == source)
            .ok_or(FrontendSessionError::MissingSource(source))
    }

    fn source_file_index(&self, source: SourceId) -> Result<usize, FrontendSessionError> {
        self.sources
            .files
            .iter()
            .position(|existing| existing.id == source)
            .ok_or(FrontendSessionError::MissingSource(source))
    }

    fn replace_source_file(&mut self, source: SourceInput) -> Result<(), FrontendSessionError> {
        let index = self.source_file_index(source.id)?;
        self.sources.files[index] = source_file_from_input(&source);
        Ok(())
    }

    fn validate_source_version(
        &self,
        source: SourceId,
        requested: SourceVersion,
    ) -> Result<(), FrontendSessionError> {
        let current = self
            .source_versions
            .get(&source)
            .copied()
            .ok_or(FrontendSessionError::MissingSource(source))?;
        if requested <= current {
            return Err(FrontendSessionError::NonIncreasingSourceVersion {
                source,
                current,
                requested,
            });
        }
        Ok(())
    }
}

fn apply_text_edits(
    source: SourceId,
    text: &mut String,
    mut edits: Vec<TextEdit>,
) -> Result<(), FrontendSessionError> {
    edits.sort_by_key(|edit| edit.start);
    for edit in edits.iter().rev() {
        if edit.start > edit.end
            || edit.end > text.len()
            || !text.is_char_boundary(edit.start)
            || !text.is_char_boundary(edit.end)
        {
            return Err(FrontendSessionError::InvalidTextEdit {
                source,
                start: edit.start,
                end: edit.end,
                len: text.len(),
            });
        }
        text.replace_range(edit.start..edit.end, &edit.replacement);
    }
    Ok(())
}
