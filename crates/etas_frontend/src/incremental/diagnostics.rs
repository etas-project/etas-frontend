use etas_core::{Diagnostic, SourceId};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DiagnosticSet {
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSet {
    pub fn by_source(&self, source: SourceId) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.primary.span.source == source)
            .collect()
    }

    pub fn sources(&self) -> Vec<SourceId> {
        dedup_sources(
            self.diagnostics
                .iter()
                .map(|diagnostic| diagnostic.primary.span.source)
                .collect(),
        )
    }
}

pub(crate) fn dedup_sources(mut sources: Vec<SourceId>) -> Vec<SourceId> {
    sources.sort_by_key(|source| source.0);
    sources.dedup();
    sources
}
