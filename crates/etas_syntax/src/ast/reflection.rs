use crate::Span;

use super::{Name, Path, TypeExpr};

#[derive(Clone, Debug, PartialEq, etas_core::RetainedSize, etas_syntax_derive::SpanNodes)]
pub struct ReflectExpr {
    pub target: ReflectTarget,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, etas_core::RetainedSize, etas_syntax_derive::SpanNodes)]
pub enum ReflectTarget {
    Path(Path),
    SpecMethod {
        receiver: Path,
        spec: Path,
        spec_args: Vec<TypeExpr>,
        method: Name,
    },
}
