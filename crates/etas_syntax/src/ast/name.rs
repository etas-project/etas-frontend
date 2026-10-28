use crate::Span;

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, etas_syntax_derive::SpanNodes)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize, etas_syntax_derive::SpanNodes)]
pub struct Path {
    pub segments: Vec<Name>,
    pub span: Span,
}
