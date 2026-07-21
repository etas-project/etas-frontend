use etas_core::Span;
use etas_hir::{
    HirArg, HirExprId, HirGenericArg, HirHandlerArmId, HirProgram, HirStmtId, ResolvedActionRef,
};

use crate::intraprocedural::{AbstractDomain, Control};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnhandledReason {
    pub hook: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnalysisStep<D> {
    Handled(D),
    Unhandled { state: D, reason: UnhandledReason },
}

impl<D> AnalysisStep<D> {
    pub fn handled(state: D) -> Self {
        Self::Handled(state)
    }

    pub fn unhandled(state: D, hook: &'static str) -> Self {
        Self::Unhandled {
            state,
            reason: UnhandledReason { hook },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandleParts<D>
where
    D: AbstractDomain,
{
    pub body: Control<D>,
    pub handler: HandlerParts<D>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandlerParts<D>
where
    D: AbstractDomain,
{
    Expr {
        expr: HirExprId,
        control: Control<D>,
    },
}

impl<D> HandleParts<D>
where
    D: AbstractDomain,
{
    pub fn into_joined_control(self) -> Control<D> {
        let mut joined = self.body;
        match self.handler {
            HandlerParts::Expr { control, .. } => {
                joined.join_assign(&control);
            }
        }
        joined
    }
}

pub trait HirAnalysisSemantics {
    type Domain: AbstractDomain;

    fn hir(&self) -> &HirProgram;

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain;

    fn unhandled_semantics(
        &mut self,
        _reason: UnhandledReason,
        state: Self::Domain,
    ) -> Self::Domain {
        self.incomplete_facts(state)
    }

    fn before_stmt(&mut self, _stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn after_stmt(&mut self, _stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn before_expr(&mut self, _expr: HirExprId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn after_expr(&mut self, _expr: HirExprId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn begin_handler_arm(&mut self, _arm: HirHandlerArmId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn perform(
        &mut self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        generic_args: &[HirGenericArg],
        args: &[HirArg],
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn direct_call(
        &mut self,
        call: HirExprId,
        callee: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn stage_compose(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn pipeline(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn try_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn handle_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn handle_expr_control(
        &mut self,
        expr: HirExprId,
        span: Span,
        _entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> Control<Self::Domain> {
        let state = parts.into_joined_control().into_joined_domain();
        match self.handle_expr(expr, span, state) {
            AnalysisStep::Handled(state) => Control::normal(state),
            AnalysisStep::Unhandled { state, reason } => {
                Control::normal(self.unhandled_semantics(reason, state))
            }
        }
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;

    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain>;
}
