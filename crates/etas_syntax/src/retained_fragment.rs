use std::sync::Arc;

use crate::{
    BodyFormatOptions, BodyParseLimits, BoundedBodyParseError, SourceBodyView, SourceFile,
    TokenStream, ast,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedFragmentForm {
    BlockContents,
    Expression,
    HandledBlock,
    HandledExpression,
}

#[derive(Debug, etas_core::RetainedSize)]
pub(crate) enum FragmentBody {
    Block(Box<ast::Block>),
    Expression(Box<ast::Expr>),
}

pub(crate) type FragmentContents = (FragmentBody, Option<ast::HandlerArg>);

/// Bounded reparse of one body envelope, without a synthetic module, function
/// header, lexical bindings, or language checking.
#[derive(Debug, etas_core::RetainedSize)]
pub struct RetainedBodyFragment {
    source: SourceFile,
    body: FragmentBody,
    handler: Option<ast::HandlerArg>,
    tokens: TokenStream,
}

impl RetainedBodyFragment {
    pub fn parse_controlled<E>(
        source: SourceFile,
        form: RetainedFragmentForm,
        limits: BodyParseLimits,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<Arc<Self>, BoundedBodyParseError<E>> {
        let parsed =
            crate::parser::parse_retained_source_bounded(&source, form, limits, checkpoint)?;
        if parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::Severity::Error)
        {
            return Err(BoundedBodyParseError::Syntax(crate::BodyParseError {
                source,
                diagnostics: parsed.diagnostics,
            }));
        }
        Ok(Arc::new(Self {
            source,
            body: parsed.value.0,
            handler: parsed.value.1,
            tokens: parsed.tokens,
        }))
    }
    pub fn body(&self) -> SourceBodyView<'_> {
        match &self.body {
            FragmentBody::Block(block) => SourceBodyView::Block(block),
            FragmentBody::Expression(expr) => SourceBodyView::Expression(expr),
        }
    }
    pub fn trailing_handler(&self) -> Option<&ast::HandlerArg> {
        self.handler.as_ref()
    }
    pub fn source(&self) -> &SourceFile {
        &self.source
    }
    pub fn format_controlled<E>(
        &self,
        options: BodyFormatOptions,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<String, crate::ControlledBodyFormatError<E>> {
        crate::retained_body::format_retained_controlled(
            &self.source,
            self.body(),
            self.trailing_handler(),
            &self.tokens.tokens,
            options,
            checkpoint,
        )
    }
}
