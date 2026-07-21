use crate::{
    Applicability, Diagnostic, Keyword, Parse, Punct, SourceFile, Span, Suggestion,
    SyntaxDiagnosticCode, TextEdit, TokenKind, ast::*, cursor::Cursor, lexer::lex_with_diagnostics,
};

pub fn parse_program(source: SourceFile) -> Parse<Program> {
    let mut diagnostics = Vec::new();
    let tokens = lex_with_diagnostics(&source, &mut diagnostics);
    diagnostics.extend(obsolete_fun_diagnostics(&source, &tokens));
    let significant = tokens.significant_tokens().cloned();
    let mut parser = Parser {
        source: &source,
        cursor: Cursor::new(significant),
        diagnostics,
        delimiters: Vec::new(),
        record_expr_enabled: true,
        handler_with_enabled: true,
        suppress_next_handler_with: false,
    };
    let value = parser.program();
    Parse {
        value,
        diagnostics: parser.diagnostics,
        tokens,
    }
}

fn obsolete_fun_diagnostics(source: &SourceFile, tokens: &crate::TokenStream) -> Vec<Diagnostic> {
    crate::grammar::recovery::obsolete_fun_spans(source.text())
        .into_iter()
        .filter(|range| {
            tokens.tokens.iter().any(|token| {
                token.kind == TokenKind::Ident
                    && token.span.range.start.to_usize() == range.start
                    && token.span.range.end.to_usize() == range.end
            })
        })
        .map(|range| {
            let span = Span::new(
                source.id,
                crate::TextRange::new(
                    crate::TextSize::new(range.start),
                    crate::TextSize::new(range.end),
                ),
            );
            Diagnostic::syntax(
                SyntaxDiagnosticCode::InvalidItem,
                span,
                "`fun` is obsolete source syntax; use `flow`",
            )
            .with_suggestion(Suggestion {
                title: "replace `fun` with `flow`".to_string(),
                edits: vec![TextEdit {
                    range: span.range,
                    replacement: "flow".to_string(),
                }],
                applicability: Applicability::MachineApplicable,
            })
        })
        .collect()
}

pub(crate) struct Parser<'a> {
    pub(crate) source: &'a SourceFile,
    pub(crate) cursor: Cursor,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) delimiters: Vec<Delimiter>,
    pub(crate) record_expr_enabled: bool,
    pub(crate) handler_with_enabled: bool,
    pub(crate) suppress_next_handler_with: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Delimiter {
    open: Punct,
    close: Punct,
    span: Span,
}

impl Parser<'_> {
    pub(crate) fn param_list(&mut self) -> Vec<Param> {
        self.expect_punct(Punct::LParen, "expected parameter list");
        let params = self.comma_list(Punct::RParen, |this| this.param());
        self.expect_punct(Punct::RParen, "expected `)` after parameters");
        params
    }

    pub(crate) fn param(&mut self) -> Param {
        let name = self.name("expected parameter name");
        self.expect_punct(Punct::Colon, "expected `:` after parameter name");
        let ty = self.type_expr();
        Param {
            span: name.span.cover(ty.span()),
            name,
            ty,
        }
    }

    pub(crate) fn arg_list(&mut self) -> Vec<Arg> {
        self.expect_punct(Punct::LParen, "expected argument list");
        let args = self.comma_list(Punct::RParen, |this| this.arg());
        self.expect_punct(Punct::RParen, "expected `)` after arguments");
        args
    }

    pub(crate) fn arg(&mut self) -> Arg {
        if self.cursor.at_ident_like() && self.cursor.nth(1).kind == TokenKind::Punct(Punct::Eq) {
            let name = self.name("expected argument name");
            self.cursor.bump();
            let value = self.expr();
            Arg::Named { name, value }
        } else {
            Arg::Positional(self.expr())
        }
    }

    pub(crate) fn path(&mut self) -> Path {
        let first = self.name("expected path segment");
        let mut span = first.span;
        let mut segments = vec![first];
        while self.cursor.eat_punct(Punct::Dot).is_some() {
            let segment = self.name("expected path segment after `.`");
            span = span.cover(segment.span);
            segments.push(segment);
        }
        Path { segments, span }
    }

    pub(crate) fn name(&mut self, message: &str) -> Name {
        if self.cursor.at_ident_like() {
            let token = self.cursor.bump();
            return Name {
                text: self.slice(token.span).to_string(),
                span: token.span,
            };
        }
        let span = self.cursor.peek().span;
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::MissingToken,
            span,
            message,
        ));
        if !self.cursor.at_eof()
            && !matches!(
                self.cursor.peek().kind,
                TokenKind::Punct(
                    Punct::RBrace | Punct::RParen | Punct::RBracket | Punct::Semi | Punct::Comma
                )
            )
        {
            self.cursor.bump();
        }
        Name {
            text: "<error>".to_string(),
            span,
        }
    }

    pub(crate) fn effect_action_ref(&mut self, message: &str) -> (EffectRef, Name) {
        let path = self.path();
        if self.cursor.at_punct(Punct::Lt) && self.effect_owner_args_are_followed_by_action() {
            let args = self.effect_args();
            let effect_span = path
                .span
                .cover(args.last().map_or(path.span, EffectArg::span));
            let effect = EffectRef {
                path,
                args,
                span: effect_span,
            };
            self.expect_punct(Punct::Dot, "expected `.` before effect action name");
            let action = self.name("expected effect action name");
            return (effect, action);
        }

        if path.segments.len() >= 2 {
            let mut segments = path.segments;
            let action = segments.pop().unwrap();
            let span = segments
                .first()
                .unwrap()
                .span
                .cover(segments.last().unwrap().span);
            return (
                EffectRef {
                    path: Path { segments, span },
                    args: Vec::new(),
                    span,
                },
                action,
            );
        }

        let diagnostic_message = if message.contains("handler action") {
            format!(
                "HandlerArmMustMatchAction: handler arm must match an effect action, not bare tag `{}`",
                path.segments
                    .iter()
                    .map(|segment| segment.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            )
        } else {
            message.to_owned()
        };
        self.diagnostics.push(Diagnostic::syntax(
            SyntaxDiagnosticCode::MissingToken,
            path.span,
            diagnostic_message,
        ));
        let action = Name {
            text: "<error>".to_string(),
            span: path.span,
        };
        (
            EffectRef {
                path: path.clone(),
                args: Vec::new(),
                span: path.span,
            },
            action,
        )
    }

    fn effect_owner_args_are_followed_by_action(&self) -> bool {
        let mut depth = 0usize;
        let mut offset = 0usize;
        loop {
            let token = self.cursor.nth(offset);
            match token.kind {
                TokenKind::Punct(Punct::Lt) => depth += 1,
                TokenKind::Punct(Punct::Gt) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self.cursor.nth(offset + 1).kind == TokenKind::Punct(Punct::Dot);
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset += 1;
        }
    }

    pub(crate) fn comma_list<T>(
        &mut self,
        terminator: Punct,
        mut parse: impl FnMut(&mut Self) -> T,
    ) -> Vec<T> {
        let mut values = Vec::new();
        if self.cursor.at_punct(terminator) {
            return values;
        }
        while !self.cursor.at_eof() && !self.cursor.at_punct(terminator) {
            let before = self.cursor.position();
            values.push(parse(self));
            if self.cursor.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.ensure_progress(before, "expected comma-list element");
        }
        values
    }

    pub(crate) fn expect_keyword(&mut self, keyword: Keyword, message: &str) -> Span {
        if let Some(token) = self.cursor.eat_keyword(keyword) {
            token.span
        } else {
            self.missing(message)
        }
    }

    pub(crate) fn expect_punct(&mut self, punct: Punct, message: &str) -> Span {
        if let Some(token) = self.cursor.eat_punct(punct) {
            self.note_delimiter_token(punct, token.span);
            token.span
        } else if is_closing_delimiter(punct) {
            self.unclosed_or_missing_delimiter(punct, message)
        } else {
            self.missing_punct(punct, message)
        }
    }

    pub(crate) fn eat_open_punct(&mut self, punct: Punct) -> Option<crate::Token> {
        let token = self.cursor.eat_punct(punct)?;
        self.note_delimiter_token(punct, token.span);
        Some(token)
    }

    pub(crate) fn note_delimiter_token(&mut self, punct: Punct, span: Span) {
        if let Some(close) = matching_close(punct) {
            self.delimiters.push(Delimiter {
                open: punct,
                close,
                span,
            });
        } else if is_closing_delimiter(punct) {
            if let Some(index) = self
                .delimiters
                .iter()
                .rposition(|delimiter| delimiter.close == punct)
            {
                self.delimiters.remove(index);
            }
        }
    }

    pub(crate) fn unclosed_or_missing_delimiter(&mut self, punct: Punct, message: &str) -> Span {
        let span = self.cursor.peek().span;
        if let Some(index) = self
            .delimiters
            .iter()
            .rposition(|delimiter| delimiter.close == punct)
        {
            let delimiter = self.delimiters.remove(index);
            let replacement = punct_text(punct).to_string();
            let diagnostic = Diagnostic::syntax(
                SyntaxDiagnosticCode::UnclosedDelimiter,
                span,
                format!("expected matching `{replacement}` before this token"),
            )
            .with_secondary(
                delimiter.span,
                format!("`{}` opened here", punct_text(delimiter.open)),
            )
            .with_suggestion(Suggestion {
                title: format!("insert missing `{replacement}`"),
                edits: vec![TextEdit {
                    range: span.range,
                    replacement,
                }],
                applicability: Applicability::HasPlaceholders,
            });
            self.diagnostics.push(diagnostic);
            span
        } else {
            self.missing_punct(punct, message)
        }
    }

    pub(crate) fn missing(&mut self, message: &str) -> Span {
        let span = self.cursor.peek().span;
        let code = if self.cursor.at_eof() {
            SyntaxDiagnosticCode::UnexpectedEof
        } else {
            SyntaxDiagnosticCode::MissingToken
        };
        self.diagnostics
            .push(Diagnostic::syntax(code, span, message.to_string()));
        span
    }

    pub(crate) fn missing_punct(&mut self, punct: Punct, message: &str) -> Span {
        let span = self.cursor.peek().span;
        let mut diagnostic = Diagnostic::syntax(
            if self.cursor.at_eof() {
                SyntaxDiagnosticCode::UnexpectedEof
            } else {
                SyntaxDiagnosticCode::MissingToken
            },
            span,
            message.to_string(),
        );
        if matches!(
            punct,
            Punct::Semi
                | Punct::RBrace
                | Punct::RParen
                | Punct::RBracket
                | Punct::LBrace
                | Punct::LParen
                | Punct::LBracket
        ) {
            diagnostic = diagnostic.with_suggestion(Suggestion {
                title: format!("insert `{}`", punct_text(punct)),
                edits: vec![TextEdit {
                    range: span.range,
                    replacement: punct_text(punct).to_string(),
                }],
                applicability: Applicability::MaybeIncorrect,
            });
        }
        self.diagnostics.push(diagnostic);
        span
    }

    pub(crate) fn bump_error(&mut self, code: SyntaxDiagnosticCode, message: &str) {
        let span = self.cursor.peek().span;
        self.diagnostics
            .push(Diagnostic::syntax(code, span, message.to_string()));
        if !self.cursor.at_eof() {
            self.cursor.bump();
        }
    }

    pub(crate) fn ensure_progress(&mut self, before: usize, message: &str) {
        if self.cursor.position() == before {
            self.bump_error(SyntaxDiagnosticCode::UnexpectedToken, message);
        }
    }

    pub(crate) fn recover_item(&mut self) {
        while !self.cursor.at_eof()
            && !self.starts_item_keyword()
            && !self.cursor.at_punct(Punct::RBrace)
        {
            self.cursor.bump();
        }
    }

    pub(crate) fn recover_stmt(&mut self) {
        while !self.cursor.at_eof()
            && !self.cursor.at_punct(Punct::Semi)
            && !self.cursor.at_punct(Punct::RBrace)
        {
            self.cursor.bump();
        }
        self.cursor.eat_punct(Punct::Semi);
    }

    pub(crate) fn recover_braced_block(&mut self) -> Span {
        let start = self.expect_punct(Punct::LBrace, "expected `{`");
        let mut depth = 1usize;
        let mut end = start;
        while !self.cursor.at_eof() && depth > 0 {
            let token = self.cursor.bump();
            end = token.span;
            match token.kind {
                TokenKind::Punct(Punct::LBrace) => depth += 1,
                TokenKind::Punct(Punct::RBrace) => depth -= 1,
                _ => {}
            }
        }
        start.cover(end)
    }

    pub(crate) fn starts_item_keyword(&self) -> bool {
        matches!(
            self.cursor.peek().kind,
            TokenKind::Keyword(
                Keyword::Type
                    | Keyword::Alias
                    | Keyword::Enum
                    | Keyword::Spec
                    | Keyword::Impl
                    | Keyword::Effect
                    | Keyword::Let
                    | Keyword::Var
                    | Keyword::Tool
                    | Keyword::Agent
                    | Keyword::Policy
                    | Keyword::Protocol
                    | Keyword::Flow
            )
        )
    }

    pub(crate) fn starts_stmt_keyword(&self) -> bool {
        matches!(
            self.cursor.peek().kind,
            TokenKind::Keyword(
                Keyword::Let
                    | Keyword::Var
                    | Keyword::If
                    | Keyword::Match
                    | Keyword::For
                    | Keyword::While
                    | Keyword::Retry
                    | Keyword::Resume
                    | Keyword::Finish
                    | Keyword::Return
                    | Keyword::Break
                    | Keyword::Continue
            )
        )
    }

    pub(crate) fn looks_like_named_field_brackets(&self) -> bool {
        self.cursor.at_punct(Punct::LBracket)
            && self.cursor.nth(1).kind.is_ident_like()
            && self.cursor.nth(2).kind == TokenKind::Punct(Punct::Eq)
    }

    pub(crate) fn previous_span(&self) -> Span {
        self.cursor.nth(0).span
    }

    pub(crate) fn slice(&self, span: Span) -> &str {
        &self.source.text()[span.range.start.to_usize()..span.range.end.to_usize()]
    }
}

fn matching_close(punct: Punct) -> Option<Punct> {
    Some(match punct {
        Punct::LParen => Punct::RParen,
        Punct::LBrace => Punct::RBrace,
        Punct::LBracket => Punct::RBracket,
        _ => return None,
    })
}

fn is_closing_delimiter(punct: Punct) -> bool {
    matches!(punct, Punct::RParen | Punct::RBrace | Punct::RBracket)
}

fn punct_text(punct: Punct) -> &'static str {
    match punct {
        Punct::LParen => "(",
        Punct::RParen => ")",
        Punct::LBrace => "{",
        Punct::RBrace => "}",
        Punct::LBracket => "[",
        Punct::RBracket => "]",
        Punct::Comma => ",",
        Punct::Dot => ".",
        Punct::Colon => ":",
        Punct::ColonColon => "::",
        Punct::Semi => ";",
        Punct::Arrow => "->",
        Punct::FatArrow => "=>",
        Punct::Pipe => "|",
        Punct::Tilde => "~",
        Punct::TildeArrow => "~>",
        Punct::Eq => "=",
        Punct::EqEq => "==",
        Punct::Bang => "!",
        Punct::BangEq => "!=",
        Punct::Lt => "<",
        Punct::LtEq => "<=",
        Punct::Gt => ">",
        Punct::GtEq => ">=",
        Punct::Plus => "+",
        Punct::Minus => "-",
        Punct::Amp => "&",
        Punct::Star => "*",
        Punct::Slash => "/",
        Punct::Percent => "%",
        Punct::AmpAmp => "&&",
        Punct::PipePipe => "||",
        Punct::Question => "?",
        Punct::Hash => "#",
        Punct::At => "@",
    }
}
