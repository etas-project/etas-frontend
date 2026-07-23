use etas_core::Span;

use crate::{
    Arena, HirBlockId, HirEffectRow, HirExprId, HirItemId, HirTypeId, HirTypeParamBound,
    ResolvedPath, ScopeId, SymbolId, TopLevelLetClassification, Visibility,
};

pub type HirItemArena = Arena<HirItemId, HirItem>;

#[derive(Clone, Debug)]
pub struct HirAnnotation {
    pub path: ResolvedPath,
    pub args: Vec<HirAnnotationArg>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirAnnotationArg {
    Positional {
        value: HirExprId,
        span: Span,
    },
    Named {
        name: String,
        name_span: Span,
        value: HirExprId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub enum HirItem {
    TypeAlias(HirTypeAliasDecl),
    Type(HirTypeDecl),
    Enum(HirEnumDecl),
    Spec(HirSpecDecl),
    Impl(HirImplDecl),
    Effect(HirEffectDecl),
    TopLevelLet(HirTopLevelLetDecl),
    Tool(HirToolDecl),
    Agent(HirAgentDecl),
    Protocol(HirProtocolDecl),
    Flow(HirFlowDecl),
    Error { span: Span },
}

impl HirItem {
    pub fn span(&self) -> Span {
        match self {
            Self::TypeAlias(item) => item.span,
            Self::Type(item) => item.span,
            Self::Enum(item) => item.span,
            Self::Spec(item) => item.span,
            Self::Impl(item) => item.span,
            Self::Effect(item) => item.span,
            Self::TopLevelLet(item) => item.span,
            Self::Tool(item) => item.span,
            Self::Agent(item) => item.span,
            Self::Protocol(item) => item.span,
            Self::Flow(item) => item.span,
            Self::Error { span } => *span,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirTypeAliasDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub target: HirTypeId,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirTypeDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub body: HirTypeDeclBody,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirTypeDeclBody {
    Bodyless,
    Representation(HirTypeId),
}

#[derive(Clone, Debug)]
pub struct HirEnumDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub variants: Vec<HirEnumVariant>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirEnumVariant {
    pub symbol: SymbolId,
    pub fields: Vec<HirTypeId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirSpecKind {
    TypeSpec,
    CallableSpec,
    TraceSpec,
}

#[derive(Clone, Debug)]
pub struct HirSpecDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub bounds: Vec<HirTypeParamBound>,
    pub kind: HirSpecKind,
    pub callable: Option<HirSpecCallableSignature>,
    pub trace: Option<HirSpecExpr>,
    pub items: Vec<HirSpecItem>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirSpecCallableSignature {
    pub input: HirTypeId,
    pub output: HirTypeId,
    pub effects: Option<HirEffectRow>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirSpecExpr {
    Atom(HirEffectRef),
    Allow {
        pattern: HirEffectRef,
        span: Span,
    },
    Deny {
        pattern: HirEffectRef,
        span: Span,
    },
    And {
        lhs: Box<HirSpecExpr>,
        rhs: Box<HirSpecExpr>,
        span: Span,
    },
    Or {
        lhs: Box<HirSpecExpr>,
        rhs: Box<HirSpecExpr>,
        span: Span,
    },
    Before {
        before: Box<HirSpecExpr>,
        after: Box<HirSpecExpr>,
        span: Span,
    },
    After {
        after: Box<HirSpecExpr>,
        before: Box<HirSpecExpr>,
        span: Span,
    },
}

impl HirSpecExpr {
    pub fn span(&self) -> Span {
        match self {
            Self::Atom(pattern) => pattern.span,
            Self::Allow { span, .. }
            | Self::Deny { span, .. }
            | Self::And { span, .. }
            | Self::Or { span, .. }
            | Self::Before { span, .. }
            | Self::After { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug)]
pub enum HirSpecItem {
    FlowSignature(Box<HirFlowSignature>),
    Error { span: Span },
}

#[derive(Clone, Debug)]
pub struct HirFlowSignature {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub return_type: Option<HirTypeId>,
    pub effects: Option<HirEffectRow>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirImplDecl {
    pub target: HirImplTarget,
    pub items: Vec<HirImplItem>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirSpecRef {
    pub spec_path: ResolvedPath,
    pub spec_args: Vec<HirTypeId>,
    pub span: Span,
}

pub type HirImplSpecRef = HirSpecRef;

#[derive(Clone, Debug)]
pub enum HirImplTarget {
    Inherent {
        target: ResolvedPath,
        type_args: Vec<HirTypeId>,
    },
    SpecSatisfaction {
        specs: Vec<HirImplSpecRef>,
        self_type: HirTypeId,
    },
    Error,
}

#[derive(Clone, Debug)]
pub enum HirImplItem {
    Flow(HirFlowDecl),
    Action(HirEffectActionDecl),
    Error { span: Span },
}

#[derive(Clone, Debug)]
pub struct HirEffectDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub extends: Option<HirEffectRef>,
    pub body: HirEffectBody,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirEffectBody {
    Empty {
        span: Span,
    },
    Block {
        actions: Vec<HirEffectActionDecl>,
        span: Span,
    },
}

impl HirEffectBody {
    pub fn actions(&self) -> &[HirEffectActionDecl] {
        match self {
            Self::Empty { .. } => &[],
            Self::Block { actions, .. } => actions,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirEffectActionDecl {
    pub symbol: SymbolId,
    pub selector_params: Vec<HirActionSelectorParam>,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub return_type: HirTypeId,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirActionSelectorParam {
    Type { symbol: SymbolId },
}

#[derive(Clone, Debug)]
pub struct HirFlowDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub return_type: Option<HirTypeId>,
    pub effects: Option<HirEffectRow>,
    pub conformances: Vec<HirDeclarationConformance>,
    pub body: HirFlowBody,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Copy, Debug)]
pub enum HirFlowBody {
    Block(HirBlockId),
    Expr {
        expr: HirExprId,
        lowered_block: HirBlockId,
        span: Span,
    },
}

impl HirFlowBody {
    pub fn block(self) -> HirBlockId {
        match self {
            Self::Block(block)
            | Self::Expr {
                lowered_block: block,
                ..
            } => block,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirDeclarationConformance {
    pub target: HirDeclarationConformanceTarget,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirDeclarationConformanceTarget {
    Path(HirSpecRef),
    InlineTraceSpec(HirSpecExpr),
    Error { span: Span },
}

#[derive(Clone, Debug)]
pub struct HirToolDecl {
    pub symbol: SymbolId,
    pub type_params: Vec<SymbolId>,
    pub params: Vec<SymbolId>,
    pub return_type: HirTypeId,
    pub effects: Option<HirEffectRow>,
    pub conformances: Vec<HirDeclarationConformance>,
    pub body: HirToolBody,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirToolBody {
    Source(HirFlowBody),
    Decl { span: Span },
    Error { span: Span },
}

#[derive(Clone, Debug)]
pub struct HirAgentDecl {
    pub symbol: SymbolId,
    pub params: Vec<SymbolId>,
    pub output_type: Option<HirTypeId>,
    pub effects: Option<HirEffectRow>,
    pub conformances: Vec<HirDeclarationConformance>,
    pub body: HirAgentBody,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirAgentBody {
    Source { block: HirBlockId },
    Decl { span: Span },
    Error { span: Span },
}

impl HirAgentBody {
    pub fn block(&self) -> Option<HirBlockId> {
        match self {
            Self::Source { block } => Some(*block),
            Self::Decl { .. } | Self::Error { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirTopLevelLetDecl {
    pub symbol: SymbolId,
    pub visibility: Visibility,
    pub type_annotation: Option<HirTypeId>,
    pub value: HirExprId,
    pub classification: TopLevelLetClassification,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirMemoryAccess {
    pub kind: etas_syntax::ast::MemoryAccessKind,
    pub path: ResolvedPath,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirProtocolDecl {
    pub symbol: SymbolId,
    pub messages: Vec<HirProtocolMsg>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirProtocolMsg {
    pub from: ResolvedPath,
    pub to: ResolvedPath,
    pub payload: HirTypeId,
    pub span: Span,
}

pub use crate::ty::HirEffectRef;
