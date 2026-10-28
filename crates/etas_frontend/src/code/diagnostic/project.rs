use std::{collections::HashMap, sync::Arc};

use etas_code::{
    CodeFingerprint,
    diagnostic::{
        DiagnosticDetails, DiagnosticLocation, DiagnosticRecord, DiagnosticSeverity,
        RelatedLocation,
    },
};
use etas_core::{Diagnostic, DiagnosticCode, LabelStyle, Severity, SourceId, Span};

use crate::{
    SourceFile,
    code::service::{CandidateBodyAnalysis, CheckReportError},
};

pub(crate) struct DiagnosticProjector<'a> {
    sources: HashMap<SourceId, &'a SourceFile>,
    identities: HashMap<SourceId, (CodeFingerprint, CodeFingerprint)>,
}

impl<'a> DiagnosticProjector<'a> {
    pub(crate) fn frozen(
        frozen: &'a crate::code::definition::FrozenDefinitionEnvironment,
    ) -> Result<Self, CheckReportError> {
        let mut sources = HashMap::new();
        for source in &frozen.checked().definition_sources.source_set().files {
            if sources.insert(source.id, source).is_some() {
                return Err(CheckReportError::IncompleteAnalysis(
                    "diagnostic source identity is duplicated",
                ));
            }
        }
        let mut projector = Self {
            sources,
            identities: HashMap::new(),
        };
        projector.add_frozen_revisions(frozen)?;
        Ok(projector)
    }
    pub(crate) fn new(analysis: &'a CandidateBodyAnalysis) -> Result<Self, CheckReportError> {
        let mut sources = HashMap::new();
        for source in analysis
            .output()
            .sources
            .as_ref()
            .into_iter()
            .flat_map(|sources| &sources.files)
        {
            if sources.insert(source.id, source).is_some() {
                return Err(CheckReportError::IncompleteAnalysis(
                    "diagnostic source identity is duplicated",
                ));
            }
        }
        let mut identities = HashMap::new();
        let (candidate, syntax) = analysis.candidate_source();
        identities.insert(candidate, (syntax.source(), syntax.content()));
        let mut projector = Self {
            sources,
            identities,
        };
        projector.add_frozen_revisions(analysis.frozen())?;
        Ok(projector)
    }

    fn add_frozen_revisions(
        &mut self,
        frozen: &crate::code::definition::FrozenDefinitionEnvironment,
    ) -> Result<(), CheckReportError> {
        for revision in frozen.checked().definition_sources.revisions().values() {
            let identity = revision.syntax.identity();
            let coordinates = (identity.source(), identity.content());
            if let Some(previous) = self
                .identities
                .insert(revision.relocated.source().id, coordinates)
                && previous != coordinates
            {
                return Err(CheckReportError::IncompleteAnalysis(
                    "native revisions have conflicting source identities",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn location(&mut self, span: Span) -> Result<DiagnosticLocation, CheckReportError> {
        let source = self
            .sources
            .get(&span.source)
            .ok_or(CheckReportError::MissingSource(span.source))?;
        let start = span.range.start.to_usize();
        let end = span.range.end.to_usize();
        if start > end
            || end > source.text.len()
            || !source.text.is_char_boundary(start)
            || !source.text.is_char_boundary(end)
        {
            return Err(CheckReportError::InvalidSpan(span));
        }
        let (source_id, revision) = *self.identities.entry(span.source).or_insert_with(|| {
            let content =
                CodeFingerprint::of_bytes("etas.code.source-content.v1", source.text.as_bytes());
            let identity = match &source.path {
                Some(path) => CodeFingerprint::of_parts(
                    "etas.code.source-identity.v1",
                    [
                        b"path".as_slice(),
                        path.as_os_str().as_encoded_bytes(),
                        content.as_bytes().as_slice(),
                    ],
                ),
                None => CodeFingerprint::of_parts(
                    "etas.code.source-identity.v1",
                    [b"anonymous".as_slice(), content.as_bytes().as_slice()],
                ),
            };
            (identity, content)
        });
        Ok(DiagnosticLocation {
            source: source_id,
            revision,
            byte_start: start,
            byte_end: end,
        })
    }

    pub(crate) fn project(
        &mut self,
        diagnostic: &Diagnostic,
    ) -> Result<DiagnosticRecord, CheckReportError> {
        let related = diagnostic
            .labels
            .iter()
            .filter(|label| label.style == LabelStyle::Secondary)
            .map(|label| {
                Ok(RelatedLocation {
                    location: self.location(label.span)?,
                    message: label.message.clone(),
                })
            })
            .collect::<Result<Vec<_>, CheckReportError>>()?;
        // These are compiler diagnostic enum tags, never parsed human messages.
        let code = match &diagnostic.code {
            DiagnosticCode::Syntax(code) => format!("syntax::{code:?}"),
            DiagnosticCode::Name(code) => format!("name::{code:?}"),
            DiagnosticCode::Type(code) => format!("type::{code:?}"),
            DiagnosticCode::Effect(code) => format!("effect::{code:?}"),
            DiagnosticCode::Analysis(code) => format!("analysis::{code:?}"),
        };
        Ok(DiagnosticRecord {
            code,
            severity: match diagnostic.severity {
                Severity::Error => DiagnosticSeverity::Error,
                Severity::Warning => DiagnosticSeverity::Warning,
                Severity::Info | Severity::Hint => DiagnosticSeverity::Note,
            },
            message: diagnostic.message.clone(),
            location: Some(self.location(diagnostic.primary.span)?),
            details: (!related.is_empty()).then(|| {
                Arc::new(DiagnosticDetails {
                    related,
                    ..Default::default()
                })
            }),
        })
    }
}
