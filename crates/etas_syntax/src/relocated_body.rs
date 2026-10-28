use crate::{SourceBodyView, SourceFile, SourceId, Span, ast};
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub enum RelocatedBodyError {
    InvalidSourceReference,
    Resource(BodyRelocationLimit),
}

#[derive(Clone, Copy, Debug)]
pub struct BodyRelocationLimits {
    pub max_source_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
}

impl Default for BodyRelocationLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024,
            max_nodes: 1_048_576,
            max_depth: crate::MAX_BODY_PARSE_RECURSION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyRelocationResource {
    SourceBytes,
    AstNodes,
    Nesting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyRelocationLimit {
    pub resource: BodyRelocationResource,
    pub limit: usize,
    pub span: Span,
}

#[derive(Debug, etas_core::RetainedSize)]
enum Body {
    Block(ast::Block),
    Expression(ast::Expr),
}

/// An immutable body overlay with a candidate-local source identity. Headers
/// and imports are not copied or reparsed; only the selected body is relocated.
#[derive(Debug, etas_core::RetainedSize)]
pub struct RelocatedBodySyntax {
    source: Arc<SourceFile>,
    body: Body,
    trailing_handler: Option<ast::HandlerArg>,
}

impl RelocatedBodySyntax {
    pub fn new(
        view: SourceBodyView<'_>,
        trailing_handler: Option<&ast::HandlerArg>,
        source: &SourceFile,
        new_source: SourceId,
        limits: BodyRelocationLimits,
    ) -> Result<Arc<Self>, RelocatedBodyError> {
        let root: &dyn crate::source_mapping::SpanNode = match view {
            SourceBodyView::Block(block) => block,
            SourceBodyView::Expression(expr) => expr,
        };
        let span = view.span();
        let end = trailing_handler
            .map(|handler| handler.span().range.end)
            .unwrap_or(span.range.end);
        if end.to_usize().saturating_sub(span.range.start.to_usize()) > limits.max_source_bytes {
            return Err(RelocatedBodyError::Resource(BodyRelocationLimit {
                resource: BodyRelocationResource::SourceBytes,
                limit: limits.max_source_bytes,
                span,
            }));
        }
        let mut roots = vec![root];
        if let Some(handler) = trailing_handler {
            roots.push(handler);
        }
        crate::source_mapping::validate(&roots, source.id, source.text.len(), view.span(), limits)?;
        let mut body = match view {
            SourceBodyView::Block(block) => Body::Block(block.clone()),
            SourceBodyView::Expression(expr) => Body::Expression(expr.clone()),
        };
        match &mut body {
            Body::Block(block) => crate::source_mapping::relocate(block, new_source),
            Body::Expression(expr) => crate::source_mapping::relocate(expr, new_source),
        }
        let mut trailing_handler = trailing_handler.cloned();
        if let Some(handler) = &mut trailing_handler {
            crate::source_mapping::relocate(handler, new_source);
        }
        let source = Arc::new(SourceFile {
            id: new_source,
            path: None,
            text: source.text.clone(),
            line_index: source.line_index.clone(),
        });
        Ok(Arc::new(Self {
            source,
            body,
            trailing_handler,
        }))
    }
    pub fn source(&self) -> &Arc<SourceFile> {
        &self.source
    }
    pub fn body(&self) -> SourceBodyView<'_> {
        match &self.body {
            Body::Block(block) => SourceBodyView::Block(block),
            Body::Expression(expr) => SourceBodyView::Expression(expr),
        }
    }
    pub fn trailing_handler(&self) -> Option<&ast::HandlerArg> {
        self.trailing_handler.as_ref()
    }
}
