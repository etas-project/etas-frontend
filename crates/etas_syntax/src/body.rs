use std::{fmt, sync::Arc};

use crate::{
    Diagnostic, Severity, SourceFile, TokenStream,
    ast::Block,
    format::{BodyFormatError, BodyFormatOptions, format_body_tokens},
    parser::{parse_body_source, parse_body_source_bounded},
};

/// Native structured syntax. Construction preserves the exact source/AST/token
/// association and rejects parser recovery objects containing errors.
#[derive(Debug, etas_core::RetainedSize)]
pub struct BodySyntax {
    source: SourceFile,
    block: Block,
    tokens: TokenStream,
    diagnostics: Vec<Diagnostic>,
}

impl BodySyntax {
    pub fn parse(source: SourceFile) -> Result<Arc<Self>, BodyParseError> {
        let parsed = parse_body_source(&source);
        Self::from_parse(source, parsed)
    }

    pub fn parse_bounded(
        source: SourceFile,
        limits: crate::BodyParseLimits,
    ) -> Result<Arc<Self>, BoundedBodyParseError> {
        Self::parse_controlled(source, limits, &|| Ok::<(), std::convert::Infallible>(()))
    }

    pub fn parse_controlled<E>(
        source: SourceFile,
        limits: crate::BodyParseLimits,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<Arc<Self>, BoundedBodyParseError<E>> {
        let parsed = parse_body_source_bounded(&source, limits, checkpoint)?;
        Self::from_parse(source, parsed).map_err(BoundedBodyParseError::Syntax)
    }

    fn from_parse(
        source: SourceFile,
        parsed: crate::Parse<Block>,
    ) -> Result<Arc<Self>, BodyParseError> {
        if parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            return Err(BodyParseError {
                source,
                diagnostics: parsed.diagnostics,
            });
        }
        Ok(Arc::new(Self {
            source,
            block: parsed.value,
            tokens: parsed.tokens,
            diagnostics: parsed.diagnostics,
        }))
    }

    pub fn source(&self) -> &SourceFile {
        &self.source
    }
    pub fn block(&self) -> &Block {
        &self.block
    }
    pub fn tokens(&self) -> &TokenStream {
        &self.tokens
    }
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn format(&self, options: BodyFormatOptions) -> Result<String, BodyFormatError> {
        format_body_tokens(&self.source, &self.tokens, options)
    }

    pub fn format_controlled<E>(
        &self,
        options: BodyFormatOptions,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<String, crate::ControlledBodyFormatError<E>> {
        crate::format::body::format_body_token_slice_controlled(
            &self.source,
            &self.tokens.tokens,
            options,
            checkpoint,
        )
    }
}

#[derive(Debug)]
pub struct BodyParseError {
    pub source: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for BodyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("body source has syntax errors")
    }
}

impl std::error::Error for BodyParseError {}

#[derive(Debug)]
pub enum BoundedBodyParseError<E = std::convert::Infallible> {
    Syntax(BodyParseError),
    Resource(crate::BodyResourceError),
    Interrupted(E),
}
