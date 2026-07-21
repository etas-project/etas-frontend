use crate::{SourceId, Span, Trivia, TriviaKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenStream {
    pub source: SourceId,
    pub tokens: Vec<Token>,
}

impl TokenStream {
    pub fn significant_tokens(&self) -> impl Iterator<Item = &Token> {
        self.tokens.iter().filter(|token| !token.kind.is_trivia())
    }

    pub fn trivia(&self) -> impl Iterator<Item = Trivia> + '_ {
        self.tokens.iter().filter_map(|token| {
            Some(Trivia {
                kind: token.kind.trivia_kind()?,
                span: token.span,
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    IntLit,
    FloatLit,
    StringLit,
    CharLit,
    Keyword(Keyword),
    Punct(Punct),
    Whitespace,
    Comment(CommentKind),
    Unknown,
    Eof,
}

impl TokenKind {
    pub fn is_trivia(&self) -> bool {
        self.trivia_kind().is_some()
    }

    pub fn is_ident_like(&self) -> bool {
        matches!(self, Self::Ident | Self::Keyword(_))
    }

    pub fn trivia_kind(&self) -> Option<TriviaKind> {
        match self {
            Self::Whitespace => Some(TriviaKind::Whitespace),
            Self::Comment(CommentKind::Line) => Some(TriviaKind::LineComment),
            Self::Comment(CommentKind::Block) => Some(TriviaKind::BlockComment),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentKind {
    Line,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Module,
    Import,
    As,
    Alias,
    Type,
    Enum,
    Spec,
    Impl,
    Where,
    Private,
    Public,
    Flow,
    Effect,
    Extends,
    Policy,
    Follows,
    Action,
    Perform,
    Handle,
    Handler,
    With,
    Resume,
    Finish,
    Tool,
    Agent,
    Allow,
    Require,
    Deny,
    Before,
    After,
    Protocol,
    Let,
    Var,
    If,
    Else,
    Match,
    For,
    In,
    While,
    Limit,
    Retry,
    Return,
    Break,
    Continue,
    True,
    False,
}

impl Keyword {
    pub fn from_ident(text: &str) -> Option<Self> {
        Some(match text {
            "module" => Self::Module,
            "import" => Self::Import,
            "as" => Self::As,
            "alias" => Self::Alias,
            "type" => Self::Type,
            "enum" => Self::Enum,
            "spec" => Self::Spec,
            "impl" => Self::Impl,
            "where" => Self::Where,
            "private" => Self::Private,
            "public" => Self::Public,
            "flow" => Self::Flow,
            "effect" => Self::Effect,
            "extends" => Self::Extends,
            "policy" => Self::Policy,
            "follows" => Self::Follows,
            "action" => Self::Action,
            "perform" => Self::Perform,
            "handle" => Self::Handle,
            "handler" => Self::Handler,
            "with" => Self::With,
            "resume" => Self::Resume,
            "finish" => Self::Finish,
            "tool" => Self::Tool,
            "agent" => Self::Agent,
            "allow" => Self::Allow,
            "require" => Self::Require,
            "deny" => Self::Deny,
            "before" => Self::Before,
            "after" => Self::After,
            "protocol" => Self::Protocol,
            "let" => Self::Let,
            "var" => Self::Var,
            "if" => Self::If,
            "else" => Self::Else,
            "match" => Self::Match,
            "for" => Self::For,
            "in" => Self::In,
            "while" => Self::While,
            "limit" => Self::Limit,
            "retry" => Self::Retry,
            "return" => Self::Return,
            "break" => Self::Break,
            "continue" => Self::Continue,
            "true" => Self::True,
            "false" => Self::False,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Punct {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Colon,
    ColonColon,
    Semi,
    Arrow,
    FatArrow,
    Pipe,
    Tilde,
    TildeArrow,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Amp,
    Star,
    Slash,
    Percent,
    AmpAmp,
    PipePipe,
    Question,
    Hash,
    At,
}
