use crate::Span;

/// Bounds native parser/AST recursion, not the language's semantic expressivity.
pub const MAX_BODY_PARSE_RECURSION: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct BodyParseLimits {
    pub max_source_bytes: usize,
    pub max_tokens: usize,
    pub max_recursion: usize,
}

impl Default for BodyParseLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024,
            max_tokens: 65_536,
            max_recursion: MAX_BODY_PARSE_RECURSION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyResourceKind {
    SourceBytes,
    Tokens,
    Recursion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyResourceError {
    pub kind: BodyResourceKind,
    pub limit: usize,
    pub span: Span,
}

pub(crate) struct BodyParseBudget {
    pub limits: BodyParseLimits,
    pub depth: usize,
    pub error: Option<BodyResourceError>,
}
