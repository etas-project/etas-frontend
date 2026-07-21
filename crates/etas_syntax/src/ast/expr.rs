use crate::{Span, ast::literal::Literal, ast::name::Path, ast::pattern::Pattern};

use super::{Block, EffectRef, GenericArg, Name, TypeExpr};

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal(Literal),
    Path(Path),
    Record(RecordExpr),
    EmptyRecordOrMap {
        span: Span,
    },
    Tuple {
        elems: Vec<Expr>,
        span: Span,
    },
    Array {
        elems: Vec<Expr>,
        span: Span,
    },
    List {
        elems: Vec<Expr>,
        span: Span,
    },
    ListCons {
        head: Box<Expr>,
        tail: Box<Expr>,
        span: Span,
    },
    EmptySequence {
        span: Span,
    },
    Map(MapExpr),
    Set {
        elems: Vec<Expr>,
        span: Span,
    },
    Range(RangeExpr),
    Call(CallExpr),
    MethodCall(MethodCallExpr),
    SpecMethodCall(SpecMethodCallExpr),
    Perform(PerformExpr),
    Handle(HandleExpr),
    Handler(HandlerExpr),
    StageCompose(StageComposeExpr),
    Pipeline(PipelineExpr),
    Field(FieldExpr),
    Index(IndexExpr),
    Slice(SliceExpr),
    Try(TryExpr),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    If(IfExpr),
    Match(MatchExpr),
    Lambda(LambdaExpr),
    Block(BlockExpr),
    Error(Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Self::Literal(lit) => lit.span(),
            Self::Path(path) => path.span,
            Self::Record(record) => record.span,
            Self::Map(map) => map.span,
            Self::Tuple { span, .. }
            | Self::Array { span, .. }
            | Self::List { span, .. }
            | Self::ListCons { span, .. }
            | Self::EmptySequence { span }
            | Self::EmptyRecordOrMap { span }
            | Self::Set { span, .. }
            | Self::Error(span) => *span,
            Self::Range(range) => range.span,
            Self::Call(call) => call.span,
            Self::MethodCall(call) => call.span,
            Self::SpecMethodCall(call) => call.span,
            Self::Perform(perform) => perform.span,
            Self::Handle(handle) => handle.span,
            Self::Handler(handler) => handler.span,
            Self::StageCompose(compose) => compose.span,
            Self::Pipeline(pipeline) => pipeline.span,
            Self::Field(field) => field.span,
            Self::Index(index) => index.span,
            Self::Slice(slice) => slice.span,
            Self::Try(try_expr) => try_expr.span,
            Self::Unary(unary) => unary.span,
            Self::Binary(binary) => binary.span,
            Self::If(if_expr) => if_expr.span,
            Self::Match(match_expr) => match_expr.span,
            Self::Lambda(lambda) => lambda.span,
            Self::Block(block) => block.block.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordExpr {
    pub path: Option<Path>,
    pub generic_args: Vec<GenericArg>,
    pub fields: Vec<FieldInit>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapExpr {
    pub entries: Vec<MapEntry>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    pub key: Expr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldInit {
    Shorthand(Name),
    Named { name: Name, value: Expr },
}

impl FieldInit {
    pub fn span(&self) -> Span {
        match self {
            Self::Shorthand(name) => name.span,
            Self::Named { name, value } => name.span.cover(value.span()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallExpr {
    pub callee: Box<Expr>,
    pub generic_args: Vec<GenericArg>,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodCallExpr {
    pub receiver: Box<Expr>,
    pub method: Name,
    pub generic_args: Vec<GenericArg>,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpecMethodCallExpr {
    pub receiver: Box<Expr>,
    pub spec_path: Path,
    pub spec_args: Vec<TypeExpr>,
    pub method: Name,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Positional(Expr),
    Named { name: Name, value: Expr },
}

impl Arg {
    pub fn span(&self) -> Span {
        match self {
            Self::Positional(expr) => expr.span(),
            Self::Named { name, value } => name.span.cover(value.span()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PerformExpr {
    pub effect: EffectRef,
    pub action: Name,
    pub generic_args: Vec<GenericArg>,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandleExpr {
    pub body: Box<Expr>,
    pub handler: HandlerArg,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandlerArg {
    Expr(Box<Expr>),
    Block(HandlerBlock),
}

impl HandlerArg {
    pub fn span(&self) -> Span {
        match self {
            Self::Expr(expr) => expr.span(),
            Self::Block(block) => block.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandlerExpr {
    pub block: HandlerBlock,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandlerBlock {
    pub arms: Vec<HandlerArm>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandlerArm {
    pub effect: EffectRef,
    pub action: Name,
    pub generic_args: Vec<GenericArg>,
    pub patterns: Vec<Pattern>,
    pub body: HandlerArmBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandlerArmBody {
    Stmt(Box<super::Stmt>),
    Block(Block),
}

impl HandlerArmBody {
    pub fn span(&self) -> Span {
        match self {
            Self::Stmt(stmt) => stmt.span(),
            Self::Block(block) => block.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageComposeExpr {
    pub stages: Vec<PipelineStage>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PipelineExpr {
    pub input: Box<Expr>,
    pub stages: Vec<PipelineStage>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PipelineStage {
    pub expr: Box<Expr>,
    pub limits: Vec<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldExpr {
    pub receiver: Box<Expr>,
    pub field: Name,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexExpr {
    pub receiver: Box<Expr>,
    pub index: Box<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SliceExpr {
    pub receiver: Box<Expr>,
    pub start: Box<Expr>,
    pub end: Box<Expr>,
    pub bounds: RangeBounds,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RangeExpr {
    pub start: Box<Expr>,
    pub end: Box<Expr>,
    pub bounds: RangeBounds,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TryExpr {
    pub expr: Box<Expr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeBounds {
    ClosedOpen,
    OpenClosed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnaryExpr {
    pub op: UnaryOp,
    pub expr: Box<Expr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BinaryExpr {
    pub lhs: Box<Expr>,
    pub op: BinaryOp,
    pub rhs: Box<Expr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    AndAnd,
    OrOr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfExpr {
    pub condition: Box<Expr>,
    pub then_branch: Block,
    pub else_branch: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchExpr {
    pub scrutinee: Box<Expr>,
    pub arms: Vec<super::MatchArm>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LambdaExpr {
    pub params: LambdaParams,
    pub body: LambdaBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LambdaParams {
    Ident(Name),
    ParamList(Vec<super::Param>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LambdaBody {
    Expr(Box<Expr>),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockExpr {
    pub block: Block,
}
