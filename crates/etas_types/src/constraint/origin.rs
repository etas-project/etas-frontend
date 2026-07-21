use etas_core::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstraintOrigin {
    pub span: Span,
}
