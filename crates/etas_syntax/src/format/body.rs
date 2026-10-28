use std::fmt;

use crate::{Keyword, Punct, SourceFile, Span, TokenKind, TokenStream};

#[derive(Clone, Copy, Debug)]
pub struct BodyFormatOptions {
    pub indent_width: u8,
    pub max_output_bytes: usize,
}

impl Default for BodyFormatOptions {
    fn default() -> Self {
        Self {
            indent_width: 4,
            max_output_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyFormatError {
    InvalidSourceReference(Span),
    OutputLimit { limit: usize },
}

#[derive(Debug)]
pub enum ControlledBodyFormatError<E> {
    Format(BodyFormatError),
    Interrupted(E),
}

impl fmt::Display for BodyFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSourceReference(span) => {
                write!(f, "body token has an invalid source reference: {span:?}")
            }
            Self::OutputLimit { limit } => {
                write!(f, "formatted body exceeds output byte limit {limit}")
            }
        }
    }
}

impl std::error::Error for BodyFormatError {}

pub(crate) fn format_body_tokens(
    source: &SourceFile,
    tokens: &TokenStream,
    options: BodyFormatOptions,
) -> Result<String, BodyFormatError> {
    format_body_token_slice(source, &tokens.tokens, options)
}

pub(crate) fn format_body_token_slice(
    source: &SourceFile,
    tokens: &[crate::Token],
    options: BodyFormatOptions,
) -> Result<String, BodyFormatError> {
    format_body_token_slice_controlled(source, tokens, options, &|| {
        Ok::<(), std::convert::Infallible>(())
    })
    .map_err(|error| match error {
        ControlledBodyFormatError::Format(error) => error,
        ControlledBodyFormatError::Interrupted(never) => match never {},
    })
}

pub(crate) fn format_body_token_slice_controlled<E>(
    source: &SourceFile,
    tokens: &[crate::Token],
    options: BodyFormatOptions,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<String, ControlledBodyFormatError<E>> {
    use ControlledBodyFormatError::{Format, Interrupted};
    checkpoint().map_err(Interrupted)?;
    let mut formatter = Formatter {
        output: String::new(),
        options,
        indent: 0,
        previous: None,
    };
    let mut index = 0;
    let mut current = next_significant(tokens, &mut index, checkpoint).map_err(Interrupted)?;
    while let Some(token) = current {
        checkpoint().map_err(Interrupted)?;
        if token.kind == TokenKind::Eof {
            break;
        }
        let text = source
            .text()
            .get(token.span.range.start.to_usize()..token.span.range.end.to_usize())
            .filter(|_| token.span.source == source.id)
            .ok_or_else(|| Format(BodyFormatError::InvalidSourceReference(token.span)))?;
        let next = next_significant(tokens, &mut index, checkpoint).map_err(Interrupted)?;
        formatter
            .token(token.kind.clone(), text, next.map(|token| &token.kind))
            .map_err(Format)?;
        current = next;
    }
    while formatter.output.ends_with([' ', '\n']) {
        checkpoint().map_err(Interrupted)?;
        formatter.output.pop();
    }
    checkpoint().map_err(Interrupted)?;
    Ok(formatter.output)
}

fn next_significant<'a, E>(
    tokens: &'a [crate::Token],
    index: &mut usize,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<Option<&'a crate::Token>, E> {
    while let Some(token) = tokens.get(*index) {
        checkpoint()?;
        *index += 1;
        if !token.kind.is_trivia() {
            return Ok(Some(token));
        }
    }
    Ok(None)
}

struct Formatter {
    output: String,
    options: BodyFormatOptions,
    indent: usize,
    previous: Option<TokenKind>,
}

impl Formatter {
    fn token(
        &mut self,
        kind: TokenKind,
        text: &str,
        next: Option<&TokenKind>,
    ) -> Result<(), BodyFormatError> {
        match &kind {
            TokenKind::Punct(Punct::LBrace) => {
                if !matches!(self.previous, None | Some(TokenKind::Punct(Punct::Hash))) {
                    self.space()?;
                }
                self.write(text)?;
                self.indent += 1;
                if next != Some(&TokenKind::Punct(Punct::RBrace)) {
                    self.newline()?;
                }
            }
            TokenKind::Punct(Punct::RBrace) => {
                self.indent = self.indent.saturating_sub(1);
                if self.previous != Some(TokenKind::Punct(Punct::LBrace)) {
                    self.newline()?;
                }
                self.write(text)?;
            }
            TokenKind::Punct(Punct::Semi) => {
                self.trim_spaces();
                self.write(text)?;
                if next != Some(&TokenKind::Eof) {
                    self.newline()?;
                }
            }
            TokenKind::Punct(Punct::Comma) => {
                self.trim_spaces();
                self.write(text)?;
                self.space()?;
            }
            TokenKind::Punct(Punct::Colon) => {
                self.trim_spaces();
                self.write(text)?;
                self.space()?;
            }
            TokenKind::Punct(
                Punct::Dot | Punct::ColonColon | Punct::Question | Punct::RParen | Punct::RBracket,
            ) => {
                self.trim_spaces();
                self.write(text)?;
            }
            TokenKind::Punct(Punct::LParen | Punct::LBracket) => {
                if matches!(
                    self.previous,
                    Some(TokenKind::Keyword(
                        Keyword::Return | Keyword::If | Keyword::Match
                    ))
                ) {
                    self.space()?;
                }
                self.write(text)?;
            }
            _ => {
                if self.previous == Some(TokenKind::Punct(Punct::RBrace))
                    && !matches!(kind, TokenKind::Keyword(Keyword::Else | Keyword::With))
                    && matches!(kind, TokenKind::Ident | TokenKind::Keyword(_))
                {
                    self.newline()?;
                }
                if !matches!(
                    self.previous,
                    None | Some(TokenKind::Punct(
                        Punct::LParen
                            | Punct::LBracket
                            | Punct::Dot
                            | Punct::ColonColon
                            | Punct::Caret
                            | Punct::Bang
                            | Punct::Hash
                    ))
                ) {
                    self.space()?;
                }
                self.write(text)?;
            }
        }
        self.previous = Some(kind);
        Ok(())
    }

    fn write(&mut self, text: &str) -> Result<(), BodyFormatError> {
        if self.output.ends_with('\n') {
            let count = self
                .indent
                .checked_mul(usize::from(self.options.indent_width))
                .ok_or(BodyFormatError::OutputLimit {
                    limit: self.options.max_output_bytes,
                })?;
            self.check_growth(count)?;
            self.output.extend(std::iter::repeat_n(' ', count));
        }
        self.check_growth(text.len())?;
        self.output.push_str(text);
        Ok(())
    }

    fn check_growth(&self, bytes: usize) -> Result<(), BodyFormatError> {
        if self
            .output
            .len()
            .checked_add(bytes)
            .is_none_or(|length| length > self.options.max_output_bytes)
        {
            return Err(BodyFormatError::OutputLimit {
                limit: self.options.max_output_bytes,
            });
        }
        Ok(())
    }

    fn trim_spaces(&mut self) {
        while self.output.ends_with(' ') {
            self.output.pop();
        }
    }

    fn space(&mut self) -> Result<(), BodyFormatError> {
        if !self.output.is_empty() && !self.output.ends_with([' ', '\n']) {
            self.write(" ")?;
        }
        Ok(())
    }

    fn newline(&mut self) -> Result<(), BodyFormatError> {
        self.trim_spaces();
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.check_growth(1)?;
            self.output.push('\n');
        }
        Ok(())
    }
}
