use etas_core::Span;

use crate::{Arena, HirLiteral, HirPatId, ResolvedPath, SymbolId};

pub type HirPatArena = Arena<HirPatId, HirPat>;

#[derive(Clone, Debug)]
pub enum HirPat {
    Binding {
        symbol: SymbolId,
        span: Span,
    },
    Wildcard {
        span: Span,
    },
    Literal(HirLiteral),
    Tuple {
        elems: Vec<HirPatId>,
        span: Span,
    },
    Record {
        path: Option<ResolvedPath>,
        fields: Vec<HirRecordPatField>,
        span: Span,
    },
    Variant {
        path: ResolvedPath,
        args: Vec<HirPatId>,
        span: Span,
    },
    Error {
        span: Span,
    },
}

impl HirPat {
    pub fn span(&self) -> Span {
        match self {
            Self::Binding { span, .. }
            | Self::Wildcard { span }
            | Self::Tuple { span, .. }
            | Self::Record { span, .. }
            | Self::Variant { span, .. }
            | Self::Error { span } => *span,
            Self::Literal(literal) => literal.span(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirRecordPatField {
    pub name: String,
    pub pat: Option<HirPatId>,
    pub span: Span,
}
