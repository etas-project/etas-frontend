use crate::{
    BodyFormatError, BodyFormatOptions, BodyParseLimits, Parse, Punct, SourceFile, Span, Token,
    TokenKind, ast,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProgramBodySelection {
    pub item: usize,
    pub impl_member: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub enum SourceBodyView<'a> {
    Block(&'a ast::Block),
    Expression(&'a ast::Expr),
}

impl SourceBodyView<'_> {
    pub fn span(self) -> Span {
        match self {
            Self::Block(block) => block.span,
            Self::Expression(expr) => expr.span(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RetainedBodyError {
    InvalidProgram,
    MissingBody,
    InvalidSourceReference,
    Resource(crate::BodyResourceError),
}

/// A body view into one immutable parsed program. No AST clone, token clone,
/// pretty-print/reparse, or generated wrapper is needed to retain source code.
#[derive(Debug, etas_core::RetainedSize)]
pub struct SourceBodySyntax {
    source: Arc<SourceFile>,
    program: Arc<Parse<ast::Program>>,
    selection: ProgramBodySelection,
    token_start: usize,
    token_end: usize,
    content: Span,
}

impl SourceBodySyntax {
    pub fn new(
        source: impl Into<Arc<SourceFile>>,
        program: Arc<Parse<ast::Program>>,
        selection: ProgramBodySelection,
        limits: BodyParseLimits,
    ) -> Result<Arc<Self>, RetainedBodyError> {
        let source = source.into();
        if program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::Severity::Error)
            || program.value.span.source != source.id
            || program.tokens.source != source.id
        {
            return Err(RetainedBodyError::InvalidProgram);
        }
        let body = select(&program.value, selection)?;
        let span = body.span();
        if span.source != source.id
            || source
                .text()
                .get(span.range.start.to_usize()..span.range.end.to_usize())
                .is_none()
        {
            return Err(RetainedBodyError::InvalidSourceReference);
        }
        let tokens = &program.tokens.tokens;
        let first = tokens.partition_point(|token| token.span.range.start < span.range.start);
        let end = tokens.partition_point(|token| {
            token.span.range.end <= span.range.end && token.kind != TokenKind::Eof
        });
        let (token_start, token_end, content) = if let Some(handler) =
            selected_flow(&program.value, selection).and_then(|flow| flow.trailing_handler.as_ref())
        {
            let end_span = handler.span();
            if end_span.source != source.id || end_span.range.end > program.value.span.range.end {
                return Err(RetainedBodyError::InvalidSourceReference);
            }
            let end = tokens.partition_point(|token| {
                token.span.range.end <= end_span.range.end && token.kind != TokenKind::Eof
            });
            (
                first,
                end,
                Span::new(
                    source.id,
                    crate::TextRange::new(span.range.start, end_span.range.end),
                ),
            )
        } else {
            match body {
                SourceBodyView::Block(_) => {
                    let Some(left) = tokens.get(first) else {
                        return Err(RetainedBodyError::InvalidSourceReference);
                    };
                    let Some(right) = end.checked_sub(1).and_then(|index| tokens.get(index)) else {
                        return Err(RetainedBodyError::InvalidSourceReference);
                    };
                    if left.kind != TokenKind::Punct(Punct::LBrace)
                        || right.kind != TokenKind::Punct(Punct::RBrace)
                        || left.span.range.start != span.range.start
                        || right.span.range.end != span.range.end
                    {
                        return Err(RetainedBodyError::InvalidSourceReference);
                    }
                    (
                        first + 1,
                        end - 1,
                        Span::new(
                            source.id,
                            crate::TextRange::new(left.span.range.end, right.span.range.start),
                        ),
                    )
                }
                SourceBodyView::Expression(_) => (first, end, span),
            }
        };
        if token_end < token_start || tokens.get(token_start..token_end).is_none() {
            return Err(RetainedBodyError::InvalidSourceReference);
        }
        for (kind, actual, limit) in [
            (
                crate::BodyResourceKind::SourceBytes,
                content.range.end.to_usize() - content.range.start.to_usize(),
                limits.max_source_bytes,
            ),
            (
                crate::BodyResourceKind::Tokens,
                token_end - token_start,
                limits.max_tokens,
            ),
        ] {
            if actual > limit {
                return Err(RetainedBodyError::Resource(crate::BodyResourceError {
                    kind,
                    limit,
                    span: content,
                }));
            }
        }
        Ok(Arc::new(Self {
            source,
            program,
            selection,
            token_start,
            token_end,
            content,
        }))
    }

    pub fn body(&self) -> Result<SourceBodyView<'_>, RetainedBodyError> {
        select(&self.program.value, self.selection)
    }
    pub fn trailing_handler(&self) -> Option<&ast::HandlerArg> {
        selected_flow(&self.program.value, self.selection)
            .and_then(|flow| flow.trailing_handler.as_ref())
    }
    pub fn source(&self) -> &SourceFile {
        &self.source
    }
    pub fn program(&self) -> &Arc<Parse<ast::Program>> {
        &self.program
    }
    pub fn content_span(&self) -> Span {
        self.content
    }
    pub fn tokens(&self) -> &[Token] {
        &self.program.tokens.tokens[self.token_start..self.token_end]
    }
    pub fn format(&self, options: BodyFormatOptions) -> Result<String, BodyFormatError> {
        self.format_controlled(options, &|| Ok::<(), std::convert::Infallible>(()))
            .map_err(|error| match error {
                crate::ControlledBodyFormatError::Format(error) => error,
                crate::ControlledBodyFormatError::Interrupted(never) => match never {},
            })
    }

    pub fn format_controlled<E>(
        &self,
        options: BodyFormatOptions,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<String, crate::ControlledBodyFormatError<E>> {
        let body = self.body().map_err(|_| {
            crate::ControlledBodyFormatError::Format(BodyFormatError::InvalidSourceReference(
                self.content,
            ))
        })?;
        format_retained_controlled(
            &self.source,
            body,
            self.trailing_handler(),
            self.tokens(),
            options,
            checkpoint,
        )
    }
}

pub(crate) fn format_retained_controlled<E>(
    source: &SourceFile,
    body: SourceBodyView<'_>,
    handler: Option<&ast::HandlerArg>,
    tokens: &[Token],
    options: BodyFormatOptions,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<String, crate::ControlledBodyFormatError<E>> {
    use crate::ControlledBodyFormatError::{Format, Interrupted};
    let limit = options.max_output_bytes;
    checkpoint().map_err(Interrupted)?;
    let format_tokens = |tokens: &[Token], max_output_bytes| {
        crate::format::body::format_body_token_slice_controlled(
            source,
            tokens,
            BodyFormatOptions {
                max_output_bytes,
                ..options
            },
            checkpoint,
        )
        .map_err(|error| match error {
            Format(BodyFormatError::OutputLimit { .. }) => {
                Format(BodyFormatError::OutputLimit { limit })
            }
            error => error,
        })
    };
    let Some(handler) = handler else {
        return format_tokens(tokens, limit);
    };
    if matches!(body, SourceBodyView::Block(_)) {
        let remaining = limit
            .checked_sub("handle ".len())
            .ok_or_else(|| Format(BodyFormatError::OutputLimit { limit }))?;
        let rendered = format_tokens(tokens, remaining)?;
        checkpoint().map_err(Interrupted)?;
        return Ok(format!("handle {rendered}"));
    }
    let remaining = limit
        .checked_sub("handle {  } with ".len())
        .ok_or_else(|| Format(BodyFormatError::OutputLimit { limit }))?;
    let slice = |span: Span| {
        let start = tokens.partition_point(|token| token.span.range.start < span.range.start);
        let end = tokens.partition_point(|token| {
            token.span.range.end <= span.range.end && token.kind != TokenKind::Eof
        });
        tokens
            .get(start..end)
            .ok_or_else(|| Format(BodyFormatError::InvalidSourceReference(span)))
    };
    let rendered_body = format_tokens(slice(body.span())?, remaining)?;
    let rendered_handler = format_tokens(slice(handler.span())?, remaining - rendered_body.len())?;
    checkpoint().map_err(Interrupted)?;
    Ok(format!(
        "handle {{ {rendered_body} }} with {rendered_handler}"
    ))
}

fn select(
    program: &ast::Program,
    selection: ProgramBodySelection,
) -> Result<SourceBodyView<'_>, RetainedBodyError> {
    let item = program
        .items
        .get(selection.item)
        .ok_or(RetainedBodyError::MissingBody)?;
    if let Some(member) = selection.impl_member {
        let ast::Item::Impl(decl) = &item.item else {
            return Err(RetainedBodyError::MissingBody);
        };
        let Some(ast::ImplItem::Flow(flow)) = decl.items.get(member) else {
            return Err(RetainedBodyError::MissingBody);
        };
        return Ok(flow_body(&flow.body));
    }
    match &item.item {
        ast::Item::Flow(flow) => Ok(flow_body(&flow.body)),
        ast::Item::Agent(agent) => match &agent.body {
            ast::AgentBody::Source(block) => Ok(SourceBodyView::Block(block)),
            _ => Err(RetainedBodyError::MissingBody),
        },
        ast::Item::Tool(tool) => match &tool.body {
            ast::ToolBody::Source(body) => Ok(flow_body(body)),
            _ => Err(RetainedBodyError::MissingBody),
        },
        _ => Err(RetainedBodyError::MissingBody),
    }
}

fn flow_body(body: &ast::FlowBody) -> SourceBodyView<'_> {
    match body {
        ast::FlowBody::Block(block) => SourceBodyView::Block(block),
        ast::FlowBody::Expr { expr, .. } => SourceBodyView::Expression(expr),
    }
}

fn selected_flow(
    program: &ast::Program,
    selection: ProgramBodySelection,
) -> Option<&ast::FlowDecl> {
    let item = &program.items.get(selection.item)?.item;
    match (item, selection.impl_member) {
        (ast::Item::Flow(flow), None) => Some(flow),
        (ast::Item::Impl(decl), Some(index)) => match decl.items.get(index)? {
            ast::ImplItem::Flow(flow) => Some(flow),
            _ => None,
        },
        _ => None,
    }
}
