use crate::{Keyword, Punct, Token, TokenKind};

#[derive(Clone)]
pub struct Cursor<'a> {
    tokens: Vec<Token>,
    position: usize,
    checkpoint: Option<&'a dyn Fn() -> bool>,
    halted: std::cell::Cell<bool>,
}

impl<'a> Cursor<'a> {
    pub fn new(tokens: impl IntoIterator<Item = Token>) -> Self {
        Self {
            tokens: tokens.into_iter().collect(),
            position: 0,
            checkpoint: None,
            halted: std::cell::Cell::new(false),
        }
    }

    pub fn with_checkpoint(
        tokens: impl IntoIterator<Item = Token>,
        checkpoint: &'a dyn Fn() -> bool,
    ) -> Self {
        Self {
            checkpoint: Some(checkpoint),
            ..Self::new(tokens)
        }
    }

    fn index(&self, offset: usize) -> usize {
        let last = self.tokens.len().saturating_sub(1);
        if self.halted.get() {
            return last;
        }
        if self.checkpoint.is_some_and(|checkpoint| !checkpoint()) {
            self.halted.set(true);
            return last;
        }
        self.position.saturating_add(offset).min(last)
    }

    pub fn peek(&self) -> &Token {
        &self.tokens[self.index(0)]
    }

    pub fn nth(&self, n: usize) -> &Token {
        &self.tokens[self.index(n)]
    }

    pub fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if self.position + 1 < self.tokens.len() {
            self.position += 1;
        }
        token
    }

    pub fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    pub fn at_keyword(&self, keyword: Keyword) -> bool {
        self.peek().kind == TokenKind::Keyword(keyword)
    }

    pub fn at_punct(&self, punct: Punct) -> bool {
        self.peek().kind == TokenKind::Punct(punct)
    }

    pub fn eat_keyword(&mut self, keyword: Keyword) -> Option<Token> {
        self.at_keyword(keyword).then(|| self.bump())
    }

    pub fn eat_punct(&mut self, punct: Punct) -> Option<Token> {
        self.at_punct(punct).then(|| self.bump())
    }

    pub fn at_ident_like(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Ident | TokenKind::Keyword(_))
    }

    pub fn position(&self) -> usize {
        self.position
    }
}
