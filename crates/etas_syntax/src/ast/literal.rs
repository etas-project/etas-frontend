use crate::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool { value: bool, span: Span },
    Int { text: String, span: Span },
    Float { text: String, span: Span },
    String { value: String, span: Span },
    Char { value: char, span: Span },
}

impl Literal {
    pub fn span(&self) -> Span {
        match self {
            Self::Bool { span, .. }
            | Self::Int { span, .. }
            | Self::Float { span, .. }
            | Self::String { span, .. }
            | Self::Char { span, .. } => *span,
        }
    }
}
