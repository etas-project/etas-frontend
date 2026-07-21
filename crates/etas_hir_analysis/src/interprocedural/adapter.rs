use etas_core::Span;
use etas_hir::{
    HirArg, HirExprId, HirGenericArg, HirHandlerArmId, HirProgram, HirStmtId, ResolvedActionRef,
};

use crate::intraprocedural::{
    AnalysisStep, Control, HandleParts, HirAnalysisSemantics, UnhandledReason,
};

use super::{
    AnalysisPhase, CallGraph, CallSite, CallTarget, InterproceduralDiagnostic,
    InterproceduralSemantics, ResolvedCallee, SummaryStore, UnitContext,
};

pub(crate) struct CallGraphCollectionAdapter<'a, S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    semantics: &'a mut S,
    context: UnitContext<S::Unit>,
    graph: &'a mut CallGraph<S::Unit>,
}

impl<'a, S> CallGraphCollectionAdapter<'a, S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    pub(crate) fn new(
        semantics: &'a mut S,
        unit: S::Unit,
        graph: &'a mut CallGraph<S::Unit>,
    ) -> Self {
        Self {
            semantics,
            context: UnitContext::new(unit, AnalysisPhase::DiscoverCalls),
            graph,
        }
    }
}

pub(crate) struct SummaryApplicationAdapter<'a, S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    semantics: &'a mut S,
    context: UnitContext<S::Unit>,
    summaries: &'a SummaryStore<S::Unit, S::Summary>,
    diagnostics: &'a mut Vec<InterproceduralDiagnostic<S::Unit>>,
}

impl<'a, S> SummaryApplicationAdapter<'a, S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    pub(crate) fn new(
        semantics: &'a mut S,
        unit: S::Unit,
        summaries: &'a SummaryStore<S::Unit, S::Summary>,
        diagnostics: &'a mut Vec<InterproceduralDiagnostic<S::Unit>>,
    ) -> Self {
        Self {
            semantics,
            context: UnitContext::new(unit, AnalysisPhase::SolveSummaries),
            summaries,
            diagnostics,
        }
    }
}

impl<S> HirAnalysisSemantics for CallGraphCollectionAdapter<'_, S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    type Domain = <S as InterproceduralSemantics>::Domain;

    fn hir(&self) -> &HirProgram {
        <S as HirAnalysisSemantics>::hir(self.semantics)
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        state
    }

    fn unhandled_semantics(
        &mut self,
        _reason: UnhandledReason,
        state: Self::Domain,
    ) -> Self::Domain {
        state
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

    fn perform(
        &mut self,
        _expr: HirExprId,
        _action: &ResolvedActionRef,
        _generic_args: &[HirGenericArg],
        _args: &[HirArg],
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn direct_call(
        &mut self,
        call: HirExprId,
        callee: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        match <S as InterproceduralSemantics>::call_target(
            self.semantics,
            self.context,
            call,
            callee,
            &state,
        ) {
            CallTarget::Direct(callee_unit) => {
                self.graph.add_call(self.context.unit, callee_unit);
            }
            CallTarget::Dynamic | CallTarget::External | CallTarget::Incomplete => {}
        }
        let _site = CallSite::<S::Unit>::unresolved(call, callee, span);
        AnalysisStep::handled(state)
    }

    fn method_call(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn stage_compose(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn pipeline(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn try_expr(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn handle_expr(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn handle_expr_control(
        &mut self,
        expr: HirExprId,
        _span: Span,
        _entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> Control<Self::Domain> {
        let _ = expr;
        parts.into_joined_control()
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if let Some(unit) = <S as InterproceduralSemantics>::anonymous_flow_dependency(
            self.semantics,
            self.context,
            expr,
        ) {
            self.graph.add_call(self.context.unit, unit);
        }
        AnalysisStep::handled(state)
    }

    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let handler_expr_arms = match self.hir().exprs.get(expr) {
            Some(etas_hir::HirExpr::Handler { handlers, .. }) => handlers.clone(),
            _ => Vec::new(),
        };
        if !handler_expr_arms.is_empty() {
            for arm in handler_expr_arms {
                if let Some(unit) = <S as InterproceduralSemantics>::handler_arm_dependency(
                    self.semantics,
                    self.context,
                    arm,
                ) {
                    self.graph.add_call(self.context.unit, unit);
                }
            }
        }
        AnalysisStep::handled(state)
    }
}

impl<S> HirAnalysisSemantics for SummaryApplicationAdapter<'_, S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    type Domain = <S as InterproceduralSemantics>::Domain;

    fn hir(&self) -> &HirProgram {
        <S as HirAnalysisSemantics>::hir(self.semantics)
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        <S as HirAnalysisSemantics>::incomplete_facts(self.semantics, state)
    }

    fn unhandled_semantics(
        &mut self,
        reason: UnhandledReason,
        state: Self::Domain,
    ) -> Self::Domain {
        <S as HirAnalysisSemantics>::unhandled_semantics(self.semantics, reason, state)
    }

    fn before_stmt(&mut self, stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        <S as HirAnalysisSemantics>::before_stmt(self.semantics, stmt, state)
    }

    fn after_stmt(&mut self, stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        <S as HirAnalysisSemantics>::after_stmt(self.semantics, stmt, state)
    }

    fn before_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        <S as HirAnalysisSemantics>::before_expr(self.semantics, expr, state)
    }

    fn after_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        <S as HirAnalysisSemantics>::after_expr(self.semantics, expr, state)
    }

    fn begin_handler_arm(&mut self, arm: HirHandlerArmId, state: Self::Domain) -> Self::Domain {
        <S as HirAnalysisSemantics>::begin_handler_arm(self.semantics, arm, state)
    }

    fn perform(
        &mut self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        generic_args: &[HirGenericArg],
        args: &[HirArg],
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::perform(
            self.semantics,
            expr,
            action,
            generic_args,
            args,
            span,
            state,
        )
    }

    fn direct_call(
        &mut self,
        call: HirExprId,
        callee: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let base_site = CallSite::unresolved(call, callee, span);
        match <S as InterproceduralSemantics>::call_target(
            self.semantics,
            self.context,
            call,
            callee,
            &state,
        ) {
            CallTarget::Direct(callee_unit) => {
                let site = base_site.with_callee(ResolvedCallee::Direct(callee_unit));
                match self.summaries.get(callee_unit) {
                    Some(summary) => {
                        AnalysisStep::handled(<S as InterproceduralSemantics>::direct_call(
                            self.semantics,
                            self.context,
                            site,
                            callee_unit,
                            summary,
                            state,
                        ))
                    }
                    None => {
                        push_unique(
                            self.diagnostics,
                            InterproceduralDiagnostic::MissingSummary {
                                call,
                                callee: callee_unit,
                            },
                        );
                        AnalysisStep::handled(<S as InterproceduralSemantics>::incomplete_call(
                            self.semantics,
                            self.context,
                            site,
                            state,
                        ))
                    }
                }
            }
            CallTarget::Dynamic => {
                AnalysisStep::handled(<S as InterproceduralSemantics>::dynamic_call(
                    self.semantics,
                    self.context,
                    base_site.with_callee(ResolvedCallee::Dynamic),
                    state,
                ))
            }
            CallTarget::External => {
                AnalysisStep::handled(<S as InterproceduralSemantics>::external_call(
                    self.semantics,
                    self.context,
                    base_site.with_callee(ResolvedCallee::External),
                    state,
                ))
            }
            CallTarget::Incomplete => {
                AnalysisStep::handled(<S as InterproceduralSemantics>::incomplete_call(
                    self.semantics,
                    self.context,
                    base_site,
                    state,
                ))
            }
        }
    }

    fn method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::method_call(self.semantics, expr, span, state)
    }

    fn stage_compose(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::stage_compose(self.semantics, expr, span, state)
    }

    fn pipeline(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::pipeline(self.semantics, expr, span, state)
    }

    fn try_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::try_expr(self.semantics, expr, span, state)
    }

    fn handle_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::handle_expr(self.semantics, expr, span, state)
    }

    fn handle_expr_control(
        &mut self,
        expr: HirExprId,
        span: Span,
        entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> Control<Self::Domain> {
        <S as HirAnalysisSemantics>::handle_expr_control(self.semantics, expr, span, entry, parts)
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::lambda_boundary(self.semantics, expr, span, state)
    }

    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        <S as HirAnalysisSemantics>::handler_boundary(self.semantics, expr, span, state)
    }
}

fn push_unique<T>(items: &mut Vec<T>, item: T)
where
    T: PartialEq,
{
    if !items.contains(&item) {
        items.push(item);
    }
}
