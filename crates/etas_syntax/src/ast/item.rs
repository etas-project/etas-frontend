use crate::Span;

use super::ty::Visibility;
use super::{Block, EffectRef, EffectRow, Expr, Name, Path, Pattern, TypeExpr};

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub module: Option<ModuleDecl>,
    pub imports: Vec<ImportDecl>,
    pub items: Vec<AnnotatedItem>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleDecl {
    pub path: Path,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportDecl {
    pub visibility: Visibility,
    pub tree: ImportTree,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImportTree {
    Single {
        path: Path,
        alias: Option<Name>,
        span: Span,
    },
    Group {
        prefix: Path,
        items: Vec<ImportItem>,
        span: Span,
    },
    Wildcard {
        prefix: Path,
        star_span: Span,
        span: Span,
    },
    Error {
        span: Span,
    },
}

impl ImportTree {
    pub fn span(&self) -> Span {
        match self {
            Self::Single { span, .. }
            | Self::Group { span, .. }
            | Self::Wildcard { span, .. }
            | Self::Error { span } => *span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportItem {
    pub name: Name,
    pub alias: Option<Name>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotatedItem {
    pub annotations: Vec<Annotation>,
    pub item: Item,
    pub span: Span,
}

impl AnnotatedItem {
    pub fn span(&self) -> Span {
        self.span
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub path: Path,
    pub args: Vec<AnnotationArg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationArg {
    Positional(Expr),
    Named { name: Name, value: Expr, span: Span },
}

impl AnnotationArg {
    pub fn span(&self) -> Span {
        match self {
            Self::Positional(expr) => expr.span(),
            Self::Named { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Alias(TypeAliasDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
    Spec(SpecDecl),
    Impl(ImplDecl),
    Effect(EffectDecl),
    TopLevelLet(TopLevelLetDecl),
    Tool(ToolDecl),
    Agent(AgentDecl),
    Protocol(ProtocolDecl),
    Flow(FlowDecl),
    Error(ErrorItem),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Self::Alias(item) => item.span,
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
            Self::Error(item) => item.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErrorItem {
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeAliasDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub target: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub body: TypeDeclBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDeclBody {
    Bodyless,
    Representation(TypeExpr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeParam {
    pub name: Name,
    pub kind: TypeParamKind,
    pub bounds: Vec<TypeParamBound>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeParamBound {
    pub path: Path,
    pub args: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeParamKind {
    Type,
    Effect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumVariant {
    pub name: Name,
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecKind {
    TypeSpec,
    CallableSpec,
    TraceSpec,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpecDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub bounds: Vec<TypeParamBound>,
    pub kind: SpecKind,
    pub callable: Option<SpecCallableSignature>,
    pub trace: Option<SpecExpr>,
    pub items: Vec<SpecItem>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpecCallableSignature {
    pub input: TypeExpr,
    pub output: TypeExpr,
    pub effects: Option<EffectRow>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpecExpr {
    Atom(EffectRef),
    Allow {
        pattern: EffectRef,
        span: Span,
    },
    Deny {
        pattern: EffectRef,
        span: Span,
    },
    And {
        lhs: Box<SpecExpr>,
        rhs: Box<SpecExpr>,
        span: Span,
    },
    Or {
        lhs: Box<SpecExpr>,
        rhs: Box<SpecExpr>,
        span: Span,
    },
    Before {
        before: Box<SpecExpr>,
        after: Box<SpecExpr>,
        span: Span,
    },
    After {
        after: Box<SpecExpr>,
        before: Box<SpecExpr>,
        span: Span,
    },
}

impl SpecExpr {
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

#[derive(Clone, Debug, PartialEq)]
pub enum SpecItem {
    FlowSignature(Box<FlowSignature>),
    Error(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlowSignature {
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub declared_effects: Option<EffectRow>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImplDecl {
    pub target: ImplTarget,
    pub items: Vec<ImplItem>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpecRef {
    pub spec_path: Path,
    pub spec_args: Vec<TypeExpr>,
    pub span: Span,
}

pub type ImplSpecRef = SpecRef;

#[derive(Clone, Debug, PartialEq)]
pub enum ImplTarget {
    Inherent {
        target: Path,
        type_args: Vec<TypeExpr>,
        span: Span,
    },
    SpecSatisfaction {
        specs: Vec<ImplSpecRef>,
        self_type: TypeExpr,
        span: Span,
    },
    Error {
        span: Span,
    },
}

impl ImplTarget {
    pub fn span(&self) -> Span {
        match self {
            Self::Inherent { span, .. }
            | Self::SpecSatisfaction { span, .. }
            | Self::Error { span } => *span,
        }
    }

    pub fn owner_path(&self) -> Option<&Path> {
        match self {
            Self::Inherent { target, .. } => Some(target),
            Self::SpecSatisfaction { specs, .. } => specs.first().map(|spec| &spec.spec_path),
            Self::Error { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImplItem {
    Flow(Box<FlowDecl>),
    Action(Box<EffectActionDecl>),
    Error(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub extends: Option<super::EffectRef>,
    pub body: EffectBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectBody {
    Empty {
        span: Span,
    },
    Block {
        actions: Vec<EffectActionDecl>,
        span: Span,
    },
}

impl EffectBody {
    pub fn actions(&self) -> &[EffectActionDecl] {
        match self {
            Self::Empty { .. } => &[],
            Self::Block { actions, .. } => actions,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Empty { span } | Self::Block { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectActionDecl {
    pub name: Name,
    pub selector_params: Vec<ActionSelectorParam>,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActionSelectorParam {
    Type(TypeParam),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlowDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub declared_effects: Option<EffectRow>,
    pub conformances: Vec<DeclarationConformance>,
    pub body: FlowBody,
    pub trailing_handler: Option<super::expr::HandlerArg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FlowBody {
    Block(Block),
    Expr { expr: Expr, span: Span },
}

impl FlowBody {
    pub fn span(&self) -> Span {
        match self {
            Self::Block(block) => block.span,
            Self::Expr { span, .. } => *span,
        }
    }

    pub fn as_block(&self) -> Option<&Block> {
        match self {
            Self::Block(block) => Some(block),
            Self::Expr { .. } => None,
        }
    }
}

impl std::ops::Deref for FlowBody {
    type Target = Block;

    fn deref(&self) -> &Self::Target {
        self.as_block()
            .expect("expression-bodied flow does not have an AST block body")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclarationConformance {
    pub target: DeclarationConformanceTarget,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeclarationConformanceTarget {
    Path(SpecRef),
    InlineTraceSpec(SpecExpr),
    Error(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDecl {
    pub visibility: Visibility,
    pub path: Path,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub effects: Option<EffectRow>,
    pub conformances: Vec<DeclarationConformance>,
    pub body: ToolBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolBody {
    Source(FlowBody),
    Decl { semicolon_span: Span },
    Error(Span),
}

impl ToolBody {
    pub fn span(&self) -> Span {
        match self {
            Self::Source(body) => body.span(),
            Self::Decl { semicolon_span } | Self::Error(semicolon_span) => *semicolon_span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub params: Vec<Param>,
    pub output_type: Option<TypeExpr>,
    pub effects: Option<EffectRow>,
    pub conformances: Vec<DeclarationConformance>,
    pub body: AgentBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentBody {
    Source(Block),
    Decl { semicolon_span: Span },
    Error(Span),
}

impl AgentBody {
    pub fn span(&self) -> Span {
        match self {
            Self::Source(body) => body.span,
            Self::Decl { semicolon_span } | Self::Error(semicolon_span) => *semicolon_span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopLevelLetDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub type_annotation: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryAccessKind {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryAccess {
    pub kind: MemoryAccessKind,
    pub path: Path,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolDecl {
    pub visibility: Visibility,
    pub name: Name,
    pub messages: Vec<ProtocolMsg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolMsg {
    pub from: Path,
    pub to: Path,
    pub payload: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: Name,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProtocolMsgEndpoint {
    pub path: Path,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatternList {
    pub patterns: Vec<Pattern>,
}
