use std::collections::BTreeSet;

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirArg, HirExpr, HirExprId, HirGenericArg, HirHandlerArmId, HirItem, HirItemId, HirProgram,
    ImportAliasOrigin, ResolveResult, ResolvedActionRef, SymbolDef, SymbolId, SymbolKind,
};
use etas_hir_analysis::HirAnalysisContext;
use etas_hir_analysis::interprocedural::{
    CallSite, CallTarget, HirAnalysisBody, InterproceduralSemantics, UnitContext,
};
use etas_hir_analysis::intraprocedural::{
    AnalysisStep, Control, HandleParts, HandlerParts, HirAnalysisSemantics,
};

use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};
use crate::{EffectFacts, EffectRegistry, EffectSummary, EffectUnit};

use super::domain::{TraceSpecState, TraceSpecSummary};

pub struct TraceSpecSemantics<'a> {
    hir: &'a HirProgram,
    context: HirAnalysisContext,
    types: &'a etas_types::TypeOutput,
    facts: &'a EffectFacts,
    trace_spec_items: BTreeSet<HirItemId>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_materialization_errors: Vec<crate::EffectPipelineError>,
    rejected_items: Vec<HirItemId>,
}

impl<'a> TraceSpecSemantics<'a> {
    pub fn with_context(
        hir: &'a HirProgram,
        context: HirAnalysisContext,
        types: &'a etas_types::TypeOutput,
        _registry: &'a EffectRegistry,
        facts: &'a EffectFacts,
        trace_spec_items: BTreeSet<HirItemId>,
    ) -> Self {
        Self {
            hir,
            context,
            types,
            facts,
            trace_spec_items,
            diagnostics: Vec::new(),
            diagnostic_materialization_errors: Vec::new(),
            rejected_items: Vec::new(),
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        Vec<Diagnostic>,
        Vec<HirItemId>,
        Vec<crate::EffectPipelineError>,
    ) {
        (
            self.diagnostics,
            self.rejected_items,
            self.diagnostic_materialization_errors,
        )
    }

    fn reject_trace_spec_state(
        &mut self,
        owner: Option<HirItemId>,
        span: Span,
        message: impl Into<String>,
        mut state: TraceSpecState,
    ) -> TraceSpecState {
        if owner.is_some_and(|owner| self.trace_spec_items.contains(&owner)) {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                message,
            ));
        }
        if let Some(owner) = owner.filter(|owner| self.trace_spec_items.contains(owner)) {
            self.rejected_items.push(owner);
        }
        state.mark_incomplete();
        state
    }

    fn reject_trace_spec_state_at_anchor(
        &mut self,
        owner: Option<HirItemId>,
        anchor: DiagnosticAnchor,
        message: impl Into<String>,
        mut state: TraceSpecState,
    ) -> TraceSpecState {
        if owner.is_some_and(|owner| self.trace_spec_items.contains(&owner)) {
            match materialize_effect_diagnostic(
                self.hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                anchor,
                message,
            ) {
                Ok(diagnostic) => self.diagnostics.push(diagnostic),
                Err(error) => self.diagnostic_materialization_errors.push(error),
            }
        }
        if let Some(owner) = owner.filter(|owner| self.trace_spec_items.contains(owner)) {
            self.rejected_items.push(owner);
        }
        state.mark_incomplete();
        state
    }

    fn apply_effect_summary(&self, state: &mut TraceSpecState, summary: &EffectSummary) {
        state.action_trace.seq_assign(summary.action_trace.clone());
        state
            .requested_actions
            .union_assign(&summary.requested_actions);
    }

    fn apply_trace_spec_summary(&self, state: &mut TraceSpecState, summary: &TraceSpecSummary) {
        state.seq_assign(summary);
    }

    fn incomplete_summary_at_anchor(
        &mut self,
        unit: EffectUnit,
        anchor: DiagnosticAnchor,
        message: impl Into<String>,
    ) -> TraceSpecSummary {
        match materialize_effect_diagnostic(
            self.hir,
            EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            message,
        ) {
            Ok(diagnostic) => self.diagnostics.push(diagnostic),
            Err(error) => self.diagnostic_materialization_errors.push(error),
        }
        if let Some(owner) = unit.owner() {
            self.rejected_items.push(owner);
        }
        let mut summary = TraceSpecSummary::for_unit(unit);
        summary.mark_incomplete();
        summary
    }

    fn apply_expr_effects(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: TraceSpecState,
    ) -> TraceSpecState {
        let Some(summary) = self.facts.expr_effects.get(&expr) else {
            return self.reject_trace_spec_state(
                state.owner,
                span,
                "trace spec analysis requires materialized expression action facts",
                state,
            );
        };
        self.apply_effect_summary(&mut state, summary);
        state
    }

    fn call_error_message(&self, site: CallSite<EffectUnit>) -> &'static str {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(site.callee_expr) else {
            return "trace spec analysis requires a resolved direct-call target";
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return "trace spec analysis requires a resolved direct-call target";
        };
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return "trace spec analysis requires a resolved direct-call target";
        };
        match &symbol.def {
            SymbolDef::ImportAlias {
                origin: ImportAliasOrigin::SourceImport,
                ..
            } => "trace spec analysis requires a source import target for direct calls",
            SymbolDef::Item { .. } if symbol.defining_item.is_none() => {
                "trace spec analysis requires a direct source callable defining item"
            }
            _ => "trace spec analysis requires a resolved direct-call target",
        }
    }

    fn source_item_for_path(&self, path: &[String]) -> Option<HirItemId> {
        let (name, module_path) = path.split_last()?;
        for (item, hir_item) in self.hir.items.iter() {
            let Some(symbol) = item_symbol(hir_item) else {
                continue;
            };
            let Some(symbol) = self.hir.symbols.get(symbol) else {
                continue;
            };
            if symbol.name != *name {
                continue;
            }
            let Some(module) = self.hir.modules_arena.get(symbol.defining_module) else {
                continue;
            };
            let Some(module_name) = &module.name else {
                continue;
            };
            let actual = module_name
                .segments
                .iter()
                .map(|segment| segment.name.as_str())
                .collect::<Vec<_>>();
            if actual == module_path.iter().map(String::as_str).collect::<Vec<_>>() {
                return Some(item);
            }
        }
        None
    }

    fn is_typed_flow_expr(&self, expr: HirExprId) -> bool {
        self.types
            .facts
            .expr_types
            .get(&expr)
            .and_then(|ty| self.types.store.get(*ty))
            .is_some_and(|ty| matches!(ty, etas_types::Type::Function(_)))
    }

    fn is_typed_flow_symbol(&self, symbol: SymbolId) -> bool {
        self.types
            .facts
            .symbol_types
            .get(&symbol)
            .and_then(|fact| match fact {
                etas_types::SymbolTypeFact::Param { ty }
                | etas_types::SymbolTypeFact::Local { ty, .. }
                | etas_types::SymbolTypeFact::Value { ty }
                | etas_types::SymbolTypeFact::TopLevelLet { ty, .. } => Some(*ty),
                _ => None,
            })
            .and_then(|ty| self.types.store.get(ty))
            .is_some_and(|ty| matches!(ty, etas_types::Type::Function(_)))
    }
}

impl HirAnalysisSemantics for TraceSpecSemantics<'_> {
    type Domain = TraceSpecState;

    fn hir(&self) -> &HirProgram {
        self.hir
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        let anchor = state
            .unit
            .map(DiagnosticAnchor::Unit)
            .unwrap_or(DiagnosticAnchor::Project);
        self.reject_trace_spec_state_at_anchor(
            state.owner,
            anchor,
            "trace spec analysis is missing checked facts",
            state,
        )
    }

    fn perform(
        &mut self,
        expr: HirExprId,
        _action: &ResolvedActionRef,
        _generic_args: &[HirGenericArg],
        _args: &[HirArg],
        span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if let Some(fact) = self.facts.performed_actions.get(&expr) {
            let summary = fact.summary.clone();
            if summary.action_trace == crate::ActionTraceDomain::Empty
                && !summary.requested_actions.effects.is_empty()
            {
                return AnalysisStep::handled(self.reject_trace_spec_state(
                    state.owner,
                    span,
                    "trace spec analysis requires ordered performed-action trace facts",
                    state,
                ));
            }
            self.apply_effect_summary(&mut state, &summary);
            return AnalysisStep::handled(state);
        }
        AnalysisStep::handled(self.reject_trace_spec_state(
            state.owner,
            span,
            "trace spec analysis requires a materialized performed action fact",
            state,
        ))
    }

    fn direct_call(
        &mut self,
        _call: HirExprId,
        _callee: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::unhandled(state, "direct_call")
    }

    fn method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.apply_expr_effects(expr, span, state))
    }

    fn stage_compose(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.apply_expr_effects(expr, span, state))
    }

    fn pipeline(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.apply_expr_effects(expr, span, state))
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
        _expr: HirExprId,
        _span: Span,
        _entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> Control<Self::Domain> {
        match parts.handler {
            HandlerParts::Expr { control, .. } => {
                let mut handler_state = control.into_joined_domain();
                handler_state.seq_assign(&parts.body.into_joined_domain());
                Control::normal(handler_state)
            }
        }
    }

    fn lambda_boundary(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn handler_boundary(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }
}

impl InterproceduralSemantics for TraceSpecSemantics<'_> {
    type Unit = EffectUnit;
    type Domain = TraceSpecState;
    type Summary = TraceSpecSummary;

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody {
        unit.analysis_body_with_context(self.hir, &self.context)
    }

    fn begin_unit(&mut self, context: UnitContext<Self::Unit>) -> Self::Domain {
        TraceSpecState::for_unit(context.unit)
    }

    fn end_unit(
        &mut self,
        _context: UnitContext<Self::Unit>,
        exit: Control<Self::Domain>,
    ) -> Self::Summary {
        exit.into_joined_domain()
    }

    fn stabilize_summary(
        &mut self,
        _context: UnitContext<Self::Unit>,
        mut summary: Self::Summary,
        current: Option<&Self::Summary>,
    ) -> Self::Summary {
        summary.action_trace = summary
            .action_trace
            .stabilize_for_fixpoint(current.map(|summary| &summary.action_trace));
        summary
    }

    fn external_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        let Some(summary) = self.facts.unit_effects.get(&context.unit) else {
            return self.incomplete_summary_at_anchor(
                context.unit,
                DiagnosticAnchor::Unit(context.unit),
                "trace spec analysis requires external unit action summary facts",
            );
        };
        let mut trace_spec = TraceSpecSummary::for_unit(context.unit);
        trace_spec.action_trace = summary.action_trace.clone();
        trace_spec.requested_actions = summary.requested_actions.clone();
        trace_spec
    }

    fn missing_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        self.incomplete_summary_at_anchor(
            context.unit,
            DiagnosticAnchor::Unit(context.unit),
            "trace spec analysis requires solved callee action summary facts",
        )
    }

    fn call_target(
        &mut self,
        context: UnitContext<Self::Unit>,
        _call: HirExprId,
        callee: HirExprId,
        _state: &Self::Domain,
    ) -> CallTarget<Self::Unit> {
        let Some(callee_expr) = self.hir.exprs.get(callee) else {
            return CallTarget::Incomplete;
        };
        let HirExpr::Path(path) = callee_expr else {
            return if self.is_typed_flow_expr(callee) {
                CallTarget::Dynamic
            } else {
                CallTarget::External
            };
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return CallTarget::Incomplete;
        };
        let Some(symbol_data) = self.hir.symbols.get(symbol) else {
            return CallTarget::Incomplete;
        };
        match (&symbol_data.kind, &symbol_data.def) {
            (_, SymbolDef::Item { item }) => match self.hir.items.get(*item) {
                Some(HirItem::Flow(_) | HirItem::Agent(_))
                | Some(HirItem::Tool(etas_hir::HirToolDecl {
                    body: etas_hir::HirToolBody::Source(_),
                    ..
                })) => {
                    if symbol_data.defining_item.is_none() {
                        CallTarget::Incomplete
                    } else {
                        CallTarget::Direct(EffectUnit::Item(*item))
                    }
                }
                Some(HirItem::Tool(etas_hir::HirToolDecl {
                    body: etas_hir::HirToolBody::Decl { .. },
                    ..
                })) => CallTarget::External,
                Some(HirItem::TopLevelLet(_)) if self.is_typed_flow_expr(callee) => {
                    CallTarget::Direct(EffectUnit::Item(*item))
                }
                Some(_) => CallTarget::External,
                None => CallTarget::Incomplete,
            },
            (
                _,
                SymbolDef::ImportAlias {
                    path,
                    origin: ImportAliasOrigin::SourceImport,
                },
            ) if path.first().is_some_and(|segment| segment == "std") => CallTarget::External,
            (
                _,
                SymbolDef::ImportAlias {
                    path,
                    origin: ImportAliasOrigin::SourceImport,
                },
            ) => self
                .source_item_for_path(path)
                .map(|item| CallTarget::Direct(EffectUnit::Item(item)))
                .unwrap_or(CallTarget::Incomplete),
            (_, SymbolDef::ImportAlias { .. }) => CallTarget::External,
            (SymbolKind::Param, _) | (_, SymbolDef::Param { .. })
                if self.is_typed_flow_expr(callee) || self.is_typed_flow_symbol(symbol) =>
            {
                CallTarget::Dynamic
            }
            (SymbolKind::Local, _) if context.unit.owner().is_some() => CallTarget::Dynamic,
            _ if context.unit.owner().is_some() => CallTarget::Dynamic,
            _ => CallTarget::External,
        }
    }

    fn handler_arm_dependency(
        &mut self,
        context: UnitContext<Self::Unit>,
        arm: HirHandlerArmId,
    ) -> Option<Self::Unit> {
        Some(EffectUnit::HandlerArm {
            owner: context.unit.owner()?,
            arm,
        })
    }

    fn direct_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        _callee: Self::Unit,
        summary: &Self::Summary,
        mut state: Self::Domain,
    ) -> Self::Domain {
        if let Some(effect_summary) = self.facts.expr_effects.get(&site.call) {
            self.apply_effect_summary(&mut state, effect_summary);
            return state;
        }
        self.apply_trace_spec_summary(&mut state, summary);
        if summary.incomplete
            && state
                .owner
                .is_some_and(|owner| self.trace_spec_items.contains(&owner))
        {
            return self.reject_trace_spec_state(
                state.owner,
                site.span,
                "trace spec analysis requires complete callee action facts",
                state,
            );
        }
        state
    }

    fn dynamic_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        if self.facts.expr_effects.contains_key(&site.call) {
            return self.apply_expr_effects(site.call, site.span, state);
        }
        self.reject_trace_spec_state(
            state.owner,
            site.span,
            "trace spec analysis requires resolved first-class flow action facts",
            state,
        )
    }

    fn external_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.apply_expr_effects(site.call, site.span, state)
    }

    fn incomplete_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.reject_trace_spec_state(state.owner, site.span, self.call_error_message(site), state)
    }
}

fn item_symbol(item: &HirItem) -> Option<etas_hir::SymbolId> {
    match item {
        HirItem::Flow(flow) => Some(flow.symbol),
        HirItem::Tool(tool) => Some(tool.symbol),
        HirItem::Agent(agent) => Some(agent.symbol),
        HirItem::TopLevelLet(value) => Some(value.symbol),
        _ => None,
    }
}
