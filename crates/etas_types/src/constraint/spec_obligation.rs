use etas_core::Span;
use etas_hir::SymbolId;

use crate::TypeId;

#[derive(Clone, Debug)]
pub struct SpecObligation {
    pub ty: TypeId,
    pub spec_symbol: SymbolId,
    pub args: Vec<TypeId>,
    pub span: Span,
}
