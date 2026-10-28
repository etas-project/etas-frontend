use std::sync::Arc;

use etas_code::CallableId;

use crate::{
    CheckedProgram,
    code::{definition::DefinitionCatalog, descriptor::ProjectionError, draft::DefaultBodyOverlay},
};

/// Evidence of a native, single-slot transform in the same immutable lexical
/// environment. This is not derived from a user-provided report or digest.
pub(crate) struct BodyPreservation<'a> {
    overlay: &'a DefaultBodyOverlay,
    candidate: &'a CheckedProgram,
    dependency_completion: bool,
}

impl<'a> BodyPreservation<'a> {
    pub(crate) fn verify(
        overlay: &'a DefaultBodyOverlay,
        candidate: &'a CheckedProgram,
    ) -> Result<Self, ProjectionError> {
        let original = overlay.frozen().checked();
        let invalid = || {
            ProjectionError::Invalid(
                "candidate changed its frozen source archive or compilation environment".into(),
            )
        };
        if !Arc::ptr_eq(&original.std_registry, &candidate.std_registry)
            || original.compiler_version != candidate.compiler_version
            || original.project_environment_fingerprint != candidate.project_environment_fingerprint
            || original.dependency_metadata_fingerprints
                != candidate.dependency_metadata_fingerprints
            || original.compile_options.canonical_options_fingerprint()
                != candidate.compile_options.canonical_options_fingerprint()
        {
            return Err(invalid());
        }
        for source in original.definition_sources.parsed_sources().iter() {
            let before = original
                .definition_sources
                .source(source.source)
                .ok_or_else(invalid)?;
            let after = candidate
                .definition_sources
                .source(source.source)
                .ok_or_else(invalid)?;
            if !Arc::ptr_eq(&before.program, &after.program)
                || !Arc::ptr_eq(&before.file, &after.file)
                || before.kind != after.kind
            {
                return Err(invalid());
            }
        }
        let original_ids = original
            .definition_sources
            .parsed_sources()
            .iter()
            .map(|source| source.source)
            .collect::<std::collections::HashSet<_>>();
        let mut dependency_completion = false;
        for source in candidate.definition_sources.parsed_sources().iter() {
            if original_ids.contains(&source.source) {
                continue;
            }
            let unit = candidate
                .definition_sources
                .source(source.source)
                .ok_or_else(invalid)?;
            if !matches!(unit.kind, crate::SourceKind::DependencySourceOverlay { .. }) {
                return Err(invalid());
            }
            dependency_completion = true;
        }
        let before = original.definition_sources.revisions();
        let after = candidate.definition_sources.revisions();
        let expected_len = before.len() + usize::from(!before.contains_key(&overlay.declaration()));
        if after.len() != expected_len
            || !after
                .get(&overlay.declaration())
                .is_some_and(|revision| Arc::ptr_eq(revision, overlay.native_revision()))
        {
            return Err(invalid());
        }
        for (declaration, revision) in before {
            if *declaration != overlay.declaration()
                && !after
                    .get(declaration)
                    .is_some_and(|other| Arc::ptr_eq(revision, other))
            {
                return Err(invalid());
            }
        }
        Ok(Self {
            overlay,
            candidate,
            dependency_completion,
        })
    }

    pub(super) fn preserves(
        &self,
        id: &CallableId,
        catalog: &DefinitionCatalog,
    ) -> Result<bool, ProjectionError> {
        if self.dependency_completion {
            // Newly materialized implementations must pass the full behavioral
            // comparison; source text equality alone is not metadata evidence.
            return Ok(false);
        }
        if self
            .overlay
            .frozen()
            .contract()
            .header()
            .default_entry
            .as_ref()
            == Some(id)
        {
            // This bit is produced by comparing actual native source bytes and
            // body form under the archive/overlay ownership verified above.
            // No saved report or caller-provided content hash can establish it.
            return Ok(self.overlay.native_revision().unchanged);
        }
        let slot = self
            .overlay
            .frozen()
            .callable_slots()
            .get(id)
            .ok_or_else(|| ProjectionError::MissingIdentity("frozen callable body slot".into()))?;
        let item = catalog.item_owner(id).ok_or_else(|| {
            ProjectionError::MissingIdentity("candidate callable body owner".into())
        })?;
        let actual = self
            .candidate
            .definition_sources
            .body(item)
            .ok_or_else(|| {
                ProjectionError::MissingIdentity("candidate callable body slot".into())
            })?;
        // Older revisions of other methods are retained by the verified archive;
        // this transform is allowed to replace only the selected default slot.
        Ok(actual == slot && self.overlay.slot() != slot)
    }
}

#[cfg(test)]
mod tests;
