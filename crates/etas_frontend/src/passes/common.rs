use etas_core::{Span, TextSize};

use crate::ProjectContext;

pub(crate) fn frontend_span(context: &ProjectContext) -> Span {
    let source = context
        .sources
        .as_ref()
        .and_then(|sources| sources.files.first())
        .map_or(etas_core::SourceId(0), |source| source.id);
    Span::empty(source, TextSize::ZERO)
}
