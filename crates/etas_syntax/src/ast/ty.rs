use crate::{Span, ast::expr::Expr, ast::name::Path};

#[derive(Clone, Debug, PartialEq)]
pub enum TypeExpr {
    Handler(HandlerType),
    Arrow {
        effect: Option<EffectRow>,
        input: Box<TypeExpr>,
        output: Box<TypeExpr>,
        span: Span,
    },
    Primitive {
        kind: PrimitiveType,
        span: Span,
    },
    Path {
        path: Path,
        args: Vec<TypeExpr>,
        span: Span,
    },
    Record(RecordType),
    Tuple {
        elems: Vec<TypeExpr>,
        span: Span,
    },
    Refined {
        base: Box<TypeExpr>,
        predicate: Box<Expr>,
        span: Span,
    },
    Error(Span),
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            Self::Handler(handler) => handler.span,
            Self::Arrow { span, .. }
            | Self::Primitive { span, .. }
            | Self::Path { span, .. }
            | Self::Tuple { span, .. }
            | Self::Refined { span, .. }
            | Self::Error(span) => *span,
            Self::Record(record) => record.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GenericArg {
    Type(TypeExpr),
    EffectRow(EffectRow),
    Wildcard { span: Span },
}

impl GenericArg {
    pub fn span(&self) -> Span {
        match self {
            Self::Type(ty) => ty.span(),
            Self::EffectRow(row) => row.span,
            Self::Wildcard { span } => *span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandlerType {
    pub handled: EffectRow,
    pub produced: HandlerProducedEffects,
    pub result: Option<Box<TypeExpr>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandlerProducedEffects {
    Infer,
    Explicit(EffectRow),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
    Char,
    String,
    Bytes,
    Unit,
    Never,
}

impl PrimitiveType {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "i128" => Self::I128,
            "isize" => Self::Isize,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "u128" => Self::U128,
            "usize" => Self::Usize,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "char" => Self::Char,
            "string" => Self::String,
            "bytes" => Self::Bytes,
            "unit" => Self::Unit,
            "never" => Self::Never,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordType {
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub visibility: Option<Visibility>,
    pub name: crate::ast::Name,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectRow {
    pub effects: Vec<EffectRef>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectRef {
    pub path: Path,
    pub args: Vec<EffectArg>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectArg {
    Type(TypeExpr),
    Path(Path),
    Wildcard { span: Span },
    String { value: String, span: Span },
    Int { text: String, span: Span },
}

impl EffectArg {
    pub fn span(&self) -> Span {
        match self {
            Self::Type(ty) => ty.span(),
            Self::Path(path) => path.span,
            Self::Wildcard { span } | Self::String { span, .. } | Self::Int { span, .. } => *span,
        }
    }
}
