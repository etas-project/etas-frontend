use crate::{Span, ast::name::Path};

use super::{Literal, Name};

#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Ident(Name),
    Wildcard(Span),
    Literal(Literal),
    Tuple { elems: Vec<Pattern>, span: Span },
    Record(RecordPattern),
    Variant(VariantPattern),
    Error(Span),
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Self::Ident(name) => name.span,
            Self::Wildcard(span) | Self::Tuple { span, .. } | Self::Error(span) => *span,
            Self::Literal(literal) => literal.span(),
            Self::Record(record) => record.span,
            Self::Variant(variant) => variant.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordPattern {
    pub path: Option<Path>,
    pub fields: Vec<PatternField>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatternField {
    pub name: Name,
    pub pattern: Option<Pattern>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariantPattern {
    pub path: Path,
    pub patterns: Vec<Pattern>,
    pub span: Span,
}
