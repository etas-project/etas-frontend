use crate::{Keyword, Punct, Token, TokenKind};

#[derive(Clone, Debug)]
pub struct Cursor {
    tokens: Vec<Token>,
    position: usize,
}

impl Cursor {
    pub fn new(tokens: impl IntoIterator<Item = Token>) -> Self {
        Self {
            tokens: tokens.into_iter().collect(),
            position: 0,
        }
    }

    pub fn peek(&self) -> &Token {
        &self.tokens[self.position.min(self.tokens.len().saturating_sub(1))]
    }

    pub fn nth(&self, n: usize) -> &Token {
        &self.tokens[(self.position + n).min(self.tokens.len().saturating_sub(1))]
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
