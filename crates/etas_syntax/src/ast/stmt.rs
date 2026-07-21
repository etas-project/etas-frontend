use crate::{Span, ast::expr::Expr, ast::pattern::Pattern, ast::ty::TypeExpr};

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub final_expr: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let(LetStmt),
    Var(VarStmt),
    Assign(AssignStmt),
    If(IfStmt),
    Match(MatchStmt),
    For(ForStmt),
    While(WhileStmt),
    Retry(RetryStmt),
    Resume(ResumeStmt),
    Finish(FinishStmt),
    Return(ReturnStmt),
    Break(Span),
    Continue(Span),
    Expr(ExprStmt),
    Error(Span),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let(stmt) => stmt.span,
            Stmt::Var(stmt) => stmt.span,
            Stmt::Assign(stmt) => stmt.span,
            Stmt::If(stmt) => stmt.span,
            Stmt::Match(stmt) => stmt.span,
            Stmt::For(stmt) => stmt.span,
            Stmt::While(stmt) => stmt.span,
            Stmt::Retry(stmt) => stmt.span,
            Stmt::Resume(stmt) => stmt.span,
            Stmt::Finish(stmt) => stmt.span,
            Stmt::Return(stmt) => stmt.span,
            Stmt::Break(span) | Stmt::Continue(span) | Stmt::Error(span) => *span,
            Stmt::Expr(stmt) => stmt.span,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LetStmt {
    pub pattern: Pattern,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VarStmt {
    pub pattern: Pattern,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssignStmt {
    pub target: Expr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfStmt {
    pub condition: Expr,
    pub then_branch: Block,
    pub else_branch: Option<ElseBranch>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ElseBranch {
    If(Box<IfStmt>),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchStmt {
    pub scrutinee: Expr,
    pub arms: Vec<MatchArm>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: MatchArmBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MatchArmBody {
    Expr(Expr),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForStmt {
    pub pattern: Pattern,
    pub iter: Expr,
    pub limits: Vec<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WhileStmt {
    pub condition: Expr,
    pub limits: Vec<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetryStmt {
    pub limits: Vec<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResumeStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FinishStmt {
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprStmt {
    pub expr: Expr,
    pub span: Span,
}
