use super::CandidateAnalysisError;
use crate::{
    CheckedProgram,
    code::{
        definition::DefinitionCatalog, draft::DefaultBodyOverlay,
        service::NativeCodeSyntaxOperations,
    },
};

/// Checked native analysis of the original body, not candidate-produced report
/// data. This can fill unavailable observations, never replace a frozen bound.
pub(crate) struct CompletedBaseline {
    catalog: std::sync::Arc<DefinitionCatalog>,
}

pub(super) fn original_overlay(
    overlay: &DefaultBodyOverlay,
) -> Result<DefaultBodyOverlay, CandidateAnalysisError> {
    let frozen = overlay.frozen();
    let item = frozen
        .default_body()
        .ok_or_else(|| invalid("original default body is unavailable"))?;
    let archive = &frozen.checked().definition_sources;
    let slot = archive
        .body(item)
        .ok_or_else(|| invalid("original body slot is unavailable"))?;
    let source = archive
        .source(slot.source)
        .ok_or_else(|| invalid("original body source is unavailable"))?;
    let callable = frozen
        .contract()
        .entry()
        .ok_or_else(|| invalid("original default callable is unavailable"))?;
    let syntax = NativeCodeSyntaxOperations::default();
    let body = match archive.overlaid_body(item) {
        Some(revision) => revision.syntax.clone(),
        None => syntax
            .retain_source_body(
                callable.fingerprint(),
                source.file.clone(),
                source.program.clone(),
                etas_syntax::ProgramBodySelection {
                    item: slot.item.index,
                    impl_member: slot.impl_member,
                },
            )
            .map_err(|error| invalid(error.to_string()))?,
    };
    DefaultBodyOverlay::prepare(frozen.clone(), body, &syntax)
        .map_err(|error| invalid(format!("cannot prepare original baseline: {error:?}")))
}

impl CompletedBaseline {
    pub(crate) fn catalog(&self) -> &DefinitionCatalog {
        &self.catalog
    }

    pub(super) fn verify(
        overlay: &DefaultBodyOverlay,
        checked: &CheckedProgram,
        catalog: std::sync::Arc<DefinitionCatalog>,
    ) -> Result<Self, CandidateAnalysisError> {
        if !std::sync::Arc::ptr_eq(catalog.checked_program(), checked) {
            return Err(invalid(
                "baseline descriptor artifact belongs to a different checked program",
            ));
        }
        crate::code::contract::preservation::BodyPreservation::verify(overlay, checked)
            .map_err(|error| invalid(error.to_string()))?;
        let frozen = overlay.frozen();
        let item = frozen
            .default_body()
            .ok_or_else(|| invalid("baseline has no default body"))?;
        let original_revision = frozen.checked().definition_sources.overlaid_body(item);
        let same_body = match original_revision {
            Some(original) => {
                let before = &original.relocated;
                let after = overlay.relocated();
                before.source().text() == after.source().text()
                    && before.body().span().range == after.body().span().range
                    && std::mem::discriminant(&before.body())
                        == std::mem::discriminant(&after.body())
                    && before
                        .trailing_handler()
                        .map(|handler| handler.span().range)
                        == after.trailing_handler().map(|handler| handler.span().range)
            }
            None => overlay.native_revision().unchanged,
        };
        if !same_body {
            return Err(invalid("baseline body differs from the immutable original"));
        }
        for (id, expected) in frozen.contract().callables() {
            let actual = catalog
                .callable(id)
                .ok_or_else(|| invalid("baseline callable is unavailable"))?;
            if frozen
                .contract()
                .descriptors()
                .type_fingerprint(expected.signature)
                .map_err(|error| invalid(error.to_string()))?
                != catalog
                    .descriptors()
                    .type_fingerprint(actual.signature)
                    .map_err(|error| invalid(error.to_string()))?
            {
                return Err(invalid("baseline changed a frozen callable interface"));
            }
        }
        Ok(Self { catalog })
    }
}

pub(super) fn needs_completion(overlay: &DefaultBodyOverlay) -> bool {
    overlay
        .frozen()
        .contract()
        .callables()
        .any(|(_, callable)| {
            let analysis = &callable.analysis;
            analysis.effects.requested.is_none()
                || analysis.effects.escaping.is_none()
                || (analysis.determinism.is_none() && analysis.independent_determinism.is_none())
                || analysis.trace.is_none()
                || analysis.outcomes.is_none()
        })
}

fn invalid(reason: impl Into<String>) -> CandidateAnalysisError {
    CandidateAnalysisError::Pipeline {
        reason: reason.into(),
        missing_artifact: None,
    }
}
