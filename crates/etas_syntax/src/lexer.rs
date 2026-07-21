use crate::{
    Diagnostic, Keyword, Punct, SourceFile, Span, SyntaxDiagnosticCode, TextRange, TextSize, Token,
    TokenKind, TokenStream, token::CommentKind,
};

pub fn lex(source: &SourceFile) -> TokenStream {
    let mut diagnostics = Vec::new();
    lex_with_diagnostics(source, &mut diagnostics)
}

pub(crate) fn lex_with_diagnostics(
    source: &SourceFile,
    diagnostics: &mut Vec<Diagnostic>,
) -> TokenStream {
    let text = source.text();
    let mut tokens = Vec::new();
    let mut offset = 0;

    while offset < text.len() {
        let rest = &text[offset..];
        let Some(ch) = rest.chars().next() else {
            break;
        };
        let start = offset;

        if ch.is_whitespace() {
            offset += ch.len_utf8();
            while offset < text.len() {
                let next = text[offset..].chars().next().unwrap();
                if !next.is_whitespace() {
                    break;
                }
                offset += next.len_utf8();
            }
            push(&mut tokens, source, TokenKind::Whitespace, start, offset);
            continue;
        }

        if rest.starts_with("//") {
            offset += 2;
            while offset < text.len() && text.as_bytes()[offset] != b'\n' {
                offset += 1;
            }
            push(
                &mut tokens,
                source,
                TokenKind::Comment(CommentKind::Line),
                start,
                offset,
            );
            continue;
        }

        if rest.starts_with("/*") {
            offset += 2;
            while offset < text.len() && !text[offset..].starts_with("*/") {
                offset += text[offset..].chars().next().unwrap().len_utf8();
            }
            if offset < text.len() {
                offset += 2;
            } else {
                diagnostics.push(Diagnostic::lex(
                    SyntaxDiagnosticCode::UnterminatedBlockComment,
                    span(source, start, offset),
                    "unterminated block comment",
                ));
            }
            push(
                &mut tokens,
                source,
                TokenKind::Comment(CommentKind::Block),
                start,
                offset,
            );
            continue;
        }

        if is_ident_start(ch) {
            offset += ch.len_utf8();
            while offset < text.len() {
                let next = text[offset..].chars().next().unwrap();
                if !is_ident_continue(next) {
                    break;
                }
                offset += next.len_utf8();
            }
            let ident = &text[start..offset];
            let kind = Keyword::from_ident(ident).map_or(TokenKind::Ident, TokenKind::Keyword);
            push(&mut tokens, source, kind, start, offset);
            continue;
        }

        if ch.is_ascii_digit() {
            offset += ch.len_utf8();
            if ch == '0' && offset < text.len() {
                let prefix = text[offset..].chars().next().unwrap();
                if matches!(prefix, 'x' | 'X' | 'o' | 'O' | 'b' | 'B') {
                    offset += prefix.len_utf8();
                    while offset < text.len() {
                        let next = text[offset..].chars().next().unwrap();
                        if !(next.is_ascii_alphanumeric() || next == '_') {
                            break;
                        }
                        offset += next.len_utf8();
                    }
                    push(&mut tokens, source, TokenKind::IntLit, start, offset);
                    continue;
                }
            }
            while offset < text.len() {
                let next = text[offset..].chars().next().unwrap();
                if !(next.is_ascii_digit() || next == '_') {
                    break;
                }
                offset += next.len_utf8();
            }
            let mut kind = TokenKind::IntLit;
            if offset < text.len()
                && text[offset..].starts_with('.')
                && text[offset + 1..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_digit())
            {
                kind = TokenKind::FloatLit;
                offset += 1;
                while offset < text.len() {
                    let next = text[offset..].chars().next().unwrap();
                    if !(next.is_ascii_digit() || next == '_') {
                        break;
                    }
                    offset += next.len_utf8();
                }
            }
            if offset < text.len() && matches!(text[offset..].chars().next(), Some('e' | 'E')) {
                kind = TokenKind::FloatLit;
                offset += 1;
                if offset < text.len() && matches!(text[offset..].chars().next(), Some('+' | '-')) {
                    offset += 1;
                }
                while offset < text.len() {
                    let next = text[offset..].chars().next().unwrap();
                    if !(next.is_ascii_digit() || next == '_') {
                        break;
                    }
                    offset += next.len_utf8();
                }
            }
            push(&mut tokens, source, kind, start, offset);
            continue;
        }

        if ch == '"' {
            offset += 1;
            let mut terminated = false;
            while offset < text.len() {
                let next = text[offset..].chars().next().unwrap();
                offset += next.len_utf8();
                if next == '\\' && offset < text.len() {
                    offset += text[offset..].chars().next().unwrap().len_utf8();
                    continue;
                }
                if next == '"' {
                    terminated = true;
                    break;
                }
                if next == '\n' {
                    break;
                }
            }
            let span = span(source, start, offset);
            if !terminated {
                diagnostics.push(Diagnostic::lex(
                    SyntaxDiagnosticCode::UnterminatedString,
                    span,
                    "unterminated string literal",
                ));
            }
            tokens.push(Token {
                kind: TokenKind::StringLit,
                span,
            });
            continue;
        }

        if ch == '\'' {
            offset += 1;
            let mut terminated = false;
            while offset < text.len() {
                let next = text[offset..].chars().next().unwrap();
                offset += next.len_utf8();
                if next == '\\' && offset < text.len() {
                    offset += text[offset..].chars().next().unwrap().len_utf8();
                    continue;
                }
                if next == '\'' {
                    terminated = true;
                    break;
                }
                if next == '\n' {
                    break;
                }
            }
            let token_span = span(source, start, offset);
            if !terminated {
                diagnostics.push(Diagnostic::lex(
                    SyntaxDiagnosticCode::InvalidLiteral,
                    token_span,
                    "unterminated character literal",
                ));
            }
            tokens.push(Token {
                kind: TokenKind::CharLit,
                span: token_span,
            });
            continue;
        }

        let (kind, len) = punct(rest).unwrap_or((TokenKind::Unknown, ch.len_utf8()));
        offset += len;
        let token_span = span(source, start, offset);
        if kind == TokenKind::Unknown {
            diagnostics.push(Diagnostic::lex(
                SyntaxDiagnosticCode::UnexpectedToken,
                token_span,
                "unknown token",
            ));
        }
        tokens.push(Token {
            kind,
            span: token_span,
        });
    }

    let eof = Span::empty(source.id, TextSize::new(text.len()));
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: eof,
    });

    TokenStream {
        source: source.id,
        tokens,
    }
}

fn punct(rest: &str) -> Option<(TokenKind, usize)> {
    let two = [
        ("->", Punct::Arrow),
        ("=>", Punct::FatArrow),
        ("~>", Punct::TildeArrow),
        ("::", Punct::ColonColon),
        ("==", Punct::EqEq),
        ("!=", Punct::BangEq),
        ("<=", Punct::LtEq),
        (">=", Punct::GtEq),
        ("&&", Punct::AmpAmp),
        ("||", Punct::PipePipe),
    ];
    for (text, punct) in two {
        if rest.starts_with(text) {
            return Some((TokenKind::Punct(punct), text.len()));
        }
    }

    let punct = match rest.chars().next()? {
        '(' => Punct::LParen,
        ')' => Punct::RParen,
        '{' => Punct::LBrace,
        '}' => Punct::RBrace,
        '[' => Punct::LBracket,
        ']' => Punct::RBracket,
        ',' => Punct::Comma,
        '.' => Punct::Dot,
        ':' => Punct::Colon,
        ';' => Punct::Semi,
        '|' => Punct::Pipe,
        '~' => Punct::Tilde,
        '=' => Punct::Eq,
        '!' => Punct::Bang,
        '<' => Punct::Lt,
        '>' => Punct::Gt,
        '+' => Punct::Plus,
        '-' => Punct::Minus,
        '&' => Punct::Amp,
        '*' => Punct::Star,
        '/' => Punct::Slash,
        '%' => Punct::Percent,
        '?' => Punct::Question,
        '#' => Punct::Hash,
        '@' => Punct::At,
        _ => return None,
    };
    Some((TokenKind::Punct(punct), 1))
}

fn push(tokens: &mut Vec<Token>, source: &SourceFile, kind: TokenKind, start: usize, end: usize) {
    tokens.push(Token {
        kind,
        span: span(source, start, end),
    });
}

fn span(source: &SourceFile, start: usize, end: usize) -> Span {
    Span::new(
        source.id,
        TextRange::new(TextSize::new(start), TextSize::new(end)),
    )
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}
