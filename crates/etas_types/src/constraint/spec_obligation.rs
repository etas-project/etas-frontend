use crate::{CheckedSpecRef, TypeId};
use etas_core::Span;

#[derive(Clone, Debug)]
pub struct SpecObligation {
    pub ty: TypeId,
    pub spec: CheckedSpecRef,
    pub args: Vec<TypeId>,
    pub span: Span,
}
