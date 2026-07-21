use etas_core::Span;
use etas_hir::HirExprId;

use super::AnalysisUnit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedCallee<U>
where
    U: AnalysisUnit,
{
    Direct(U),
    Dynamic,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallSite<U>
where
    U: AnalysisUnit,
{
    pub call: HirExprId,
    pub callee_expr: HirExprId,
    pub callee: Option<ResolvedCallee<U>>,
    pub span: Span,
}

impl<U> CallSite<U>
where
    U: AnalysisUnit,
{
    pub fn unresolved(call: HirExprId, callee_expr: HirExprId, span: Span) -> Self {
        Self {
            call,
            callee_expr,
            callee: None,
            span,
        }
    }

    pub fn with_callee(mut self, callee: ResolvedCallee<U>) -> Self {
        self.callee = Some(callee);
        self
    }
}
