use etas_core::Span;
use etas_hir::{
    HirArg, HirExprId, HirGenericArg, HirHandlerArmId, HirPatId, HirStmtId, ResolvedActionRef,
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

    fn analysis_context(&self) -> &crate::HirAnalysisContext;

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

    /// Bind an arm's pattern after its scrutinee completes and before its body.
    fn begin_match_arm(
        &mut self,
        _expr: HirExprId,
        _scrutinee: HirExprId,
        _pattern: HirPatId,
        state: Self::Domain,
    ) -> Self::Domain {
        state
    }

    fn after_expr_control(
        &mut self,
        expr: HirExprId,
        control: Control<Self::Domain>,
    ) -> Control<Self::Domain> {
        control.map_states(|state| self.after_expr(expr, state))
    }

    /// Runs only when the return operand completes normally. Nested returns
    /// have already left that channel and must not be relabeled by an outer one.
    fn return_value(
        &mut self,
        _stmt: HirStmtId,
        _value: Option<HirExprId>,
        state: Self::Domain,
    ) -> Self::Domain {
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

    /// An invocation unit can analyze already captured stage values rather
    /// than evaluating the expressions that originally constructed them.
    fn stage_targets_are_captured(&self, _expr: HirExprId, _state: &Self::Domain) -> bool {
        false
    }

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

    fn begin_try_operand(&mut self, _expr: HirExprId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn begin_handle_body(&mut self, _expr: HirExprId, state: Self::Domain) -> Self::Domain {
        state
    }

    fn try_expr_control(
        &mut self,
        expr: HirExprId,
        span: Span,
        _entry: Self::Domain,
        control: Control<Self::Domain>,
    ) -> Control<Self::Domain> {
        control.with_normal_processed(|state| match self.try_expr(expr, span, state) {
            AnalysisStep::Handled(state) => Control::normal(state),
            AnalysisStep::Unhandled { state, reason } => {
                Control::normal(self.unhandled_semantics(reason, state))
            }
        })
    }

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
