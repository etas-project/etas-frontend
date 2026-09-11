use etas_core::Span;
use etas_syntax::ast;

use crate::{
    Arena, HirBlockId, HirExprId, HirGenericArg, HirHandlerArmId, HirPatId, HirTypeId,
    ResolvedActionRef, ResolvedPath, ScopeId, SymbolId,
};

pub type HirExprArena = Arena<HirExprId, HirExpr>;
pub type HirHandlerArmArena = Arena<HirHandlerArmId, HirHandlerArm>;
pub use ast::{BinaryOp as HirBinaryOp, UnaryOp as HirUnaryOp};

#[derive(Clone, Debug)]
pub enum HirExpr {
    Literal(HirLiteral),
    Path(ResolvedPath),
    Record(HirRecordExpr),
    EmptyRecordOrMap {
        span: Span,
    },
    Tuple {
        elems: Vec<HirExprId>,
        span: Span,
    },
    Array {
        elems: Vec<HirExprId>,
        span: Span,
    },
    List {
        elems: Vec<HirExprId>,
        span: Span,
    },
    ListCons {
        head: HirExprId,
        tail: HirExprId,
        span: Span,
    },
    EmptySequence {
        span: Span,
    },
    Map {
        entries: Vec<HirMapEntry>,
        span: Span,
    },
    Set {
        elems: Vec<HirExprId>,
        span: Span,
    },
    Range {
        start: HirExprId,
        end: HirExprId,
        bounds: HirRangeBounds,
        span: Span,
    },
    Call {
        callee: HirExprId,
        generic_args: Vec<HirGenericArg>,
        args: Vec<HirArg>,
        span: Span,
    },
    MethodCall {
        receiver: HirExprId,
        method: String,
        generic_args: Vec<HirGenericArg>,
        args: Vec<HirArg>,
        span: Span,
    },
    SpecMethodCall {
        receiver: HirExprId,
        spec_path: ResolvedPath,
        spec_args: Vec<HirTypeId>,
        method: String,
        args: Vec<HirArg>,
        span: Span,
    },
    Perform {
        action: ResolvedActionRef,
        generic_args: Vec<HirGenericArg>,
        args: Vec<HirArg>,
        span: Span,
    },
    Handler {
        handlers: Vec<HirHandlerArmId>,
        span: Span,
    },
    Handle {
        body: HirExprId,
        handler: HirExprId,
        span: Span,
    },
    StageCompose {
        stages: Vec<HirStage>,
        span: Span,
    },
    Pipeline {
        input: HirExprId,
        stages: Vec<HirStage>,
        span: Span,
    },
    Field {
        base: HirExprId,
        field: String,
        span: Span,
    },
    Index {
        base: HirExprId,
        index: HirExprId,
        span: Span,
    },
    Slice {
        base: HirExprId,
        start: HirExprId,
        end: HirExprId,
        bounds: HirRangeBounds,
        span: Span,
    },
    Try {
        expr: HirExprId,
        span: Span,
    },
    Unary {
        op: HirUnaryOp,
        expr: HirExprId,
        span: Span,
    },
    Binary {
        op: HirBinaryOp,
        lhs: HirExprId,
        rhs: HirExprId,
        span: Span,
    },
    If {
        cond: HirExprId,
        then_block: HirBlockId,
        else_branch: Option<HirElseBranch>,
        span: Span,
    },
    Match {
        scrutinee: HirExprId,
        arms: Vec<HirMatchArm>,
        span: Span,
    },
    Lambda {
        params: Vec<SymbolId>,
        body: HirLambdaBody,
        scope: ScopeId,
        span: Span,
    },
    Block(HirBlockId),
    Error {
        span: Span,
    },
}

impl HirExpr {
    pub fn span(&self, blocks: &crate::HirBlockArena) -> Span {
        match self {
            Self::Literal(lit) => lit.span(),
            Self::Path(path) => path.span,
            Self::Record(record) => record.span,
            Self::Tuple { span, .. }
            | Self::Array { span, .. }
            | Self::List { span, .. }
            | Self::ListCons { span, .. }
            | Self::EmptySequence { span }
            | Self::EmptyRecordOrMap { span }
            | Self::Map { span, .. }
            | Self::Set { span, .. }
            | Self::Range { span, .. }
            | Self::Call { span, .. }
            | Self::MethodCall { span, .. }
            | Self::SpecMethodCall { span, .. }
            | Self::Perform { span, .. }
            | Self::Handler { span, .. }
            | Self::Handle { span, .. }
            | Self::StageCompose { span, .. }
            | Self::Pipeline { span, .. }
            | Self::Field { span, .. }
            | Self::Index { span, .. }
            | Self::Slice { span, .. }
            | Self::Try { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::If { span, .. }
            | Self::Match { span, .. }
            | Self::Lambda { span, .. }
            | Self::Error { span } => *span,
            Self::Block(block) => blocks[*block].span,
        }
    }
}

#[derive(Clone, Debug)]
pub enum HirLiteral {
    Bool { value: bool, span: Span },
    Int { text: String, span: Span },
    Float { text: String, span: Span },
    String { value: String, span: Span },
    Char { value: char, span: Span },
}

impl HirLiteral {
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

#[derive(Clone, Debug)]
pub struct HirRecordExpr {
    pub path: Option<ResolvedPath>,
    pub generic_args: Vec<HirGenericArg>,
    pub fields: Vec<HirFieldInit>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirMapEntry {
    pub key: HirExprId,
    pub value: HirExprId,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HirRangeBounds {
    ClosedOpen,
    OpenClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirFieldInit {
    Shorthand {
        name: String,
        resolution: crate::ResolveResult,
        span: Span,
    },
    Named {
        name: String,
        value: HirExprId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub enum HirArg {
    Positional(HirExprId),
    Named {
        name: String,
        value: HirExprId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub struct HirHandlerArm {
    pub action: ResolvedActionRef,
    pub generic_args: Vec<HirGenericArg>,
    pub patterns: Vec<HirPatId>,
    pub body: HirBlockId,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct HirStage {
    pub expr: HirExprId,
    pub limits: Vec<HirExprId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirElseBranch {
    If(HirExprId),
    Block(HirBlockId),
}

#[derive(Clone, Debug)]
pub struct HirMatchArm {
    pub pat: HirPatId,
    pub body: HirMatchArmBody,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirMatchArmBody {
    Expr(HirExprId),
    Block(HirBlockId),
}

#[derive(Clone, Debug)]
pub enum HirLambdaBody {
    Expr(HirExprId),
    Block(HirBlockId),
}
