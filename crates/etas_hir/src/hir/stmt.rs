use etas_core::Span;

use crate::{Arena, HirBlockId, HirExprId, HirPatId, HirStmtId, HirTypeId, ScopeId};

pub type HirStmtArena = Arena<HirStmtId, HirStmt>;
pub type HirBlockArena = Arena<HirBlockId, HirBlock>;

#[derive(Clone, Debug)]
pub struct HirBlock {
    pub id: HirBlockId,
    pub stmts: Vec<HirStmtId>,
    pub final_expr: Option<HirExprId>,
    pub scope: ScopeId,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum HirStmt {
    Let {
        pat: HirPatId,
        type_annotation: Option<HirTypeId>,
        value: HirExprId,
        span: Span,
    },
    Var {
        pat: HirPatId,
        type_annotation: Option<HirTypeId>,
        value: HirExprId,
        span: Span,
    },
    Assign {
        target: HirExprId,
        value: HirExprId,
        span: Span,
    },
    If(HirExprId),
    Match(HirExprId),
    For {
        pat: HirPatId,
        iter: HirExprId,
        limits: Vec<HirExprId>,
        body: HirBlockId,
        span: Span,
    },
    While {
        cond: HirExprId,
        limits: Vec<HirExprId>,
        body: HirBlockId,
        span: Span,
    },
    Retry {
        limits: Vec<HirExprId>,
        body: HirBlockId,
        span: Span,
    },
    Resume {
        value: Option<HirExprId>,
        span: Span,
    },
    Finish {
        value: HirExprId,
        span: Span,
    },
    Return {
        value: Option<HirExprId>,
        span: Span,
    },
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    Expr {
        expr: HirExprId,
        span: Span,
    },
    Error {
        span: Span,
    },
}
