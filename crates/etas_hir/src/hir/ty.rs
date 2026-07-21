use etas_core::Span;
use etas_syntax::ast;

use crate::{Arena, HirExprId, HirTypeId, ResolvedPath, SymbolId};

pub type HirTypeArena = Arena<HirTypeId, HirType>;
pub use ast::PrimitiveType as HirPrimitiveType;

#[derive(Clone, Debug)]
pub enum HirType {
    Handler {
        handled: HirEffectRow,
        produced: HirHandlerProducedEffects,
        result: Option<HirTypeId>,
        span: Span,
    },
    Arrow {
        effect: Option<HirEffectRow>,
        input: HirTypeId,
        output: HirTypeId,
        span: Span,
    },
    Primitive {
        kind: HirPrimitiveType,
        span: Span,
    },
    Path {
        path: ResolvedPath,
        args: Vec<HirTypeId>,
        span: Span,
    },
    Record {
        fields: Vec<HirFieldDecl>,
        span: Span,
    },
    Tuple {
        elems: Vec<HirTypeId>,
        span: Span,
    },
    Refined {
        base: HirTypeId,
        predicate: HirExprId,
        span: Span,
    },
    Error {
        span: Span,
    },
}

impl HirType {
    pub fn span(&self) -> Span {
        match self {
            Self::Arrow { span, .. }
            | Self::Handler { span, .. }
            | Self::Primitive { span, .. }
            | Self::Path { span, .. }
            | Self::Record { span, .. }
            | Self::Tuple { span, .. }
            | Self::Refined { span, .. }
            | Self::Error { span } => *span,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirFieldDecl {
    pub symbol: SymbolId,
    pub ty: HirTypeId,
    pub visibility: ast::Visibility,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirTypeParamBound {
    pub path: ResolvedPath,
    pub args: Vec<HirTypeId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirEffectRow {
    pub effects: Vec<HirEffectRef>,
    pub tail: Option<ResolvedPath>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirGenericArg {
    Type(HirTypeId),
    EffectRow(HirEffectRow),
    Wildcard { span: Span },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirEffectRef {
    pub path: ResolvedPath,
    pub args: Vec<HirEffectArg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirEffectArg {
    Type(HirTypeId),
    Path(ResolvedPath),
    Wildcard { span: Span },
    String { value: String, span: Span },
    Int { text: String, span: Span },
}

#[derive(Clone, Debug)]
pub enum HirHandlerProducedEffects {
    Infer,
    Explicit(HirEffectRow),
}
