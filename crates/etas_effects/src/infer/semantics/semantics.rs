use std::collections::BTreeSet;

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirAnnotationArg, HirArg, HirExpr, HirExprId, HirGenericArg, HirHandlerArmId, HirItemId,
    HirLiteral, HirPat, HirPatId, HirProgram, HirStmt, HirStmtId, ResolveResult, ResolvedActionRef,
    SymbolDef,
};
use etas_hir_analysis::HirAnalysisContext;
use etas_hir_analysis::interprocedural::{
    CallSite, CallTarget, HirAnalysisBody, InterproceduralSemantics, UnitContext,
};
use etas_hir_analysis::intraprocedural::{AnalysisStep, HandleParts, HirAnalysisSemantics};
use etas_std::{RequirementSemantics, StdDecl, StdLimitKind};
use etas_types::{
    Assignable, EffectArgRef, ItemSignature, SymbolTypeFact, Type, TypeId, TypeOutput,
    TypeRelation, pipeline::symbols::TypeSymbolIndex,
};

use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};
use crate::{
    AGENTIC_INFER_ACTION, ActionInstanceRef, ActionTraceDomain, CoreEffect, Effect,
    EffectMaterializationInputs, EffectRegistry, EffectRow, EffectSet, EffectSummary,
    ExternalEffectSummaryMetadata, FrontendRejectionReason, InterpreterSupport, LimitKind,
    LimitRequirement, LimitValue, RequirementFact, RequirementSet, ToolProviderBindingMetadata,
    effect_var_id_from_name,
};

use super::state::EffectState;
use crate::infer::unit::{EffectAnonymousFlowBody, EffectUnit};

const EFFECT_ARG_EVAL_DEPTH_LIMIT: usize = 16;

pub(crate) struct EffectSemantics<'a> {
    pub(crate) hir: &'a HirProgram,
    pub(crate) context: HirAnalysisContext,
    pub(crate) types: &'a TypeOutput,
    pub(crate) type_symbols: TypeSymbolIndex,
    pub(crate) registry: EffectRegistry,
    pub(crate) inputs: EffectMaterializationInputs,
    pub(crate) diagnostics: Vec<Diagnostic>,
    diagnostic_materialization_errors: Vec<crate::EffectPipelineError>,
    tool_bindings: &'a [ToolProviderBindingMetadata],
    external_summaries: &'a [crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
}

impl<'a> EffectSemantics<'a> {
    pub(crate) fn with_context(
        hir: &'a HirProgram,
        context: HirAnalysisContext,
        types: &'a TypeOutput,
        registry: EffectRegistry,
        tool_bindings: &'a [ToolProviderBindingMetadata],
        external_summaries: &'a [crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
    ) -> Self {
        Self {
            hir,
            context,
            types,
            type_symbols: TypeSymbolIndex::build(hir),
            registry,
            inputs: EffectMaterializationInputs::default(),
            diagnostics: Vec::new(),
            diagnostic_materialization_errors: Vec::new(),
            tool_bindings,
            external_summaries,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        EffectRegistry,
        EffectMaterializationInputs,
        Vec<Diagnostic>,
        Vec<crate::EffectPipelineError>,
    ) {
        (
            self.registry,
            self.inputs,
            self.diagnostics,
            self.diagnostic_materialization_errors,
        )
    }

    pub(crate) fn incomplete_at(
        &mut self,
        span: Span,
        message: impl Into<String>,
        mut state: EffectState,
    ) -> EffectState {
        self.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
        state.mark_incomplete();
        state
    }

    fn incomplete_at_anchor(
        &mut self,
        anchor: DiagnosticAnchor,
        message: impl Into<String>,
        mut state: EffectState,
    ) -> EffectState {
        match materialize_effect_diagnostic(
            self.hir,
            EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            message,
        ) {
            Ok(diagnostic) => self.diagnostics.push(diagnostic),
            Err(error) => self.diagnostic_materialization_errors.push(error),
        }
        state.mark_incomplete();
        state
    }

    fn incomplete_summary_at_anchor(
        &mut self,
        anchor: DiagnosticAnchor,
        message: impl Into<String>,
    ) -> EffectSummary {
        match materialize_effect_diagnostic(
            self.hir,
            EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            message,
        ) {
            Ok(diagnostic) => self.diagnostics.push(diagnostic),
            Err(error) => self.diagnostic_materialization_errors.push(error),
        }
        let mut summary = EffectSummary::local();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::UnresolvedEffect);
        summary
    }

    fn is_unresolved_try_error(&self, ty: etas_types::TypeId) -> bool {
        matches!(self.types.store.get(ty), Some(Type::Var(_)))
    }

    fn try_error_types_match(
        &self,
        target: etas_types::TypeId,
        escaping: etas_types::TypeId,
    ) -> bool {
        if target == escaping {
            return true;
        }
        let relation = TypeRelation::new(&self.types.store);
        relation.assignable(target, escaping).is_ok()
            && relation.assignable(escaping, target).is_ok()
    }

    fn try_capture_error(
        &self,
        target_error: Option<etas_types::TypeId>,
        escaping_errors: &[etas_types::TypeId],
    ) -> Option<(etas_types::TypeId, etas_types::TypeId)> {
        let [escaping] = escaping_errors else {
            return None;
        };
        match target_error {
            Some(target) if self.is_unresolved_try_error(target) => Some((*escaping, *escaping)),
            Some(target) if self.try_error_types_match(target, *escaping) => {
                Some((target, *escaping))
            }
            Some(_) => None,
            None => Some((*escaping, *escaping)),
        }
    }

    pub(crate) fn summary_from_public_row(&self, row: EffectRow, span: Span) -> EffectSummary {
        let mut summary = EffectSummary::local();
        self.apply_public_row_to_summary(&mut summary, &row, span);
        summary
    }

    pub(crate) fn apply_public_row_to_summary(
        &self,
        summary: &mut EffectSummary,
        row: &EffectRow,
        span: Span,
    ) {
        for effect in row.effects.iter() {
            self.apply_effect_to_summary(summary, effect.clone(), span);
        }
    }

    pub(crate) fn apply_effect_to_summary(
        &self,
        summary: &mut EffectSummary,
        effect: Effect,
        _span: Span,
    ) {
        match &effect {
            Effect::Action(action) => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self
                    .registry
                    .runtime_requirement_reason_for_action_ref(action)
                {
                    summary.require_runtime(reason);
                }
            }
            Effect::AppliedAction(action) => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self
                    .registry
                    .runtime_requirement_reason_for_action_ref(&action.action)
                {
                    summary.require_runtime(reason);
                }
            }
            Effect::Tag(tag) => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                    summary.require_runtime(reason);
                }
            }
            Effect::Applied { tag, .. } => {
                summary.record_escaping_effect(effect.clone());
                if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                    summary.require_runtime(reason);
                }
            }
            Effect::Error(_) | Effect::Var(_) => {
                summary.record_escaping_effect(effect);
            }
        }
    }

    pub(crate) fn apply_runtime_support_for_row(
        &self,
        summary: &mut EffectSummary,
        row: &EffectRow,
    ) {
        for effect in row.effects.iter() {
            match effect {
                Effect::Action(action) => {
                    if let Some(reason) = self
                        .registry
                        .runtime_requirement_reason_for_action_ref(action)
                    {
                        summary.require_runtime(reason);
                    }
                }
                Effect::AppliedAction(action) => {
                    if let Some(reason) = self
                        .registry
                        .runtime_requirement_reason_for_action_ref(&action.action)
                    {
                        summary.require_runtime(reason);
                    }
                }
                Effect::Tag(tag) => {
                    if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                        summary.require_runtime(reason);
                    }
                }
                Effect::Applied { tag, .. } => {
                    if let Some(reason) = self.registry.runtime_requirement_reason(*tag) {
                        summary.require_runtime(reason);
                    }
                }
                Effect::Error(_) | Effect::Var(_) => {}
            }
        }
    }

    pub(crate) fn row_from_type_ref(&self, row: &etas_types::EffectRowRef) -> EffectRow {
        let effects = row
            .effects
            .iter()
            .filter_map(|effect| self.effect_from_type_ref(effect))
            .collect::<Vec<_>>();
        EffectRow {
            effects: EffectSet::from_iter(effects),
            open: row.tail.as_deref().map(effect_var_id_from_name),
        }
    }

    pub(crate) fn effect_from_type_ref(&self, effect: &etas_types::EffectRef) -> Option<Effect> {
        if self.is_core_error_effect_name(&effect.name) {
            if let Some(etas_types::EffectArgRef::Type(error)) = effect.args.first() {
                return Some(Effect::Error(*error));
            }
        }
        if let Some(action) = self.registry.action_by_name(&effect.name) {
            let args = effect.args.clone();
            return if args.is_empty() {
                Some(Effect::Action(action))
            } else {
                Some(Effect::AppliedAction(crate::ActionInstanceRef {
                    action,
                    args,
                }))
            };
        }
        self.registry.tag_by_name(&effect.name).map(Effect::Tag)
    }

    pub(crate) fn is_core_error_effect_name(&self, name: &str) -> bool {
        name == "Error"
            || self.registry.tag_by_name(name) == Some(crate::ERROR_TAG)
            || (name.starts_with("std.") && name.rsplit('.').next() == Some("Error"))
    }

    pub(crate) fn summary_for_item_signature(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Option<EffectSummary> {
        let signature = self.types.facts.item_signatures.get(&item)?;
        match signature {
            etas_types::ItemSignature::Flow(signature) => signature.effects.as_ref(),
            etas_types::ItemSignature::Agent(signature) => signature.effects.as_ref(),
            etas_types::ItemSignature::Tool(signature) => signature.effects.as_ref(),
            etas_types::ItemSignature::TopLevelLet(_) => None,
        }
        .map(|row| self.summary_from_public_row(self.row_from_type_ref(row), span))
        .or_else(|| match signature {
            etas_types::ItemSignature::Flow(_)
            | etas_types::ItemSignature::Agent(_)
            | etas_types::ItemSignature::Tool(_) => Some(EffectSummary::local()),
            etas_types::ItemSignature::TopLevelLet(_) => None,
        })
    }

    pub(crate) fn summary_for_item_public_effects(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Option<EffectSummary> {
        self.summary_for_item_signature(item, span)
    }

    pub(crate) fn summary_for_agent_call(
        &self,
        item: HirItemId,
        span: Span,
    ) -> Option<EffectSummary> {
        if !matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Agent(_))) {
            return None;
        }
        let mut summary = self
            .summary_for_item_public_effects(item, span)
            .unwrap_or_else(EffectSummary::local);
        if let Some(declaration_summary) = self.inputs.unit_effects.get(&EffectUnit::Item(item)) {
            summary.seq_assign(declaration_summary);
        }
        if let Some(call) = self.agent_infer_summary(item, span) {
            summary.seq_assign(&call);
        }
        self.apply_agent_annotation_requirements(item, &mut summary);
        Some(summary)
    }

    fn apply_agent_annotation_requirements(&self, item: HirItemId, summary: &mut EffectSummary) {
        let Some(annotations) = self.hir.item_annotations.get(&item) else {
            return;
        };
        let mut requirements = RequirementSet::new();
        for annotation in annotations {
            if annotation_name(annotation) != "limits" {
                continue;
            }
            for arg in &annotation.args {
                let value = match arg {
                    HirAnnotationArg::Positional { value, .. }
                    | HirAnnotationArg::Named { value, .. } => *value,
                };
                for limit in self.agent_limit_annotation_entries(value) {
                    if let Some(requirement) = self.limit_requirement_from_expr(limit) {
                        requirements.insert(RequirementFact::Limit(requirement));
                    }
                }
            }
        }
        summary.requirements.union_assign(&requirements);
    }

    fn agent_limit_annotation_entries(&self, expr: HirExprId) -> Vec<HirExprId> {
        match self.hir.exprs.get(expr) {
            Some(HirExpr::Array { elems, .. }) | Some(HirExpr::List { elems, .. }) => elems.clone(),
            _ => vec![expr],
        }
    }

    fn has_tool_binding(&self, item: HirItemId) -> bool {
        let Some(etas_hir::HirItem::Tool(tool)) = self.hir.items.get(item) else {
            return false;
        };
        let Some(tool_name) = self.exported_symbol_name(tool.symbol) else {
            return false;
        };
        self.tool_bindings
            .iter()
            .any(|binding| binding.tool == tool_name)
    }

    fn exported_symbol_name(&self, symbol: etas_hir::SymbolId) -> Option<Vec<String>> {
        let symbol_data = self.hir.symbols.get(symbol)?;
        let mut segments = self
            .hir
            .modules_arena
            .get(symbol_data.defining_module)
            .and_then(|module| module.name.as_ref())
            .map(|name| {
                name.segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        segments.push(symbol_data.name.clone());
        Some(segments)
    }

    pub(crate) fn callee_symbol(&self, callee: HirExprId) -> Option<etas_hir::SymbolId> {
        match self.hir.exprs.get(callee)? {
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(symbol) => Some(symbol),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn is_typed_flow_expr(&self, expr: HirExprId) -> bool {
        self.types
            .facts
            .expr_types
            .get(&expr)
            .and_then(|ty| self.types.store.get(*ty))
            .is_some_and(|ty| matches!(ty, Type::Function(_)))
    }

    pub(crate) fn summary_for_flow_expr_type(
        &self,
        expr: HirExprId,
        span: Span,
    ) -> Option<EffectSummary> {
        let flow = self.flow_type_for_expr(expr)?;
        let row = flow.effects.as_ref()?;
        Some(self.summary_from_public_row(self.row_from_type_ref(row), span))
    }

    pub(crate) fn flow_type_for_expr(&self, expr: HirExprId) -> Option<&etas_types::FlowType> {
        let ty = self.types.facts.expr_types.get(&expr)?;
        let Type::Function(flow) = self.types.store.get(*ty)? else {
            return None;
        };
        Some(flow)
    }

    fn is_nominal_constructor_callee(&self, callee: HirExprId) -> bool {
        if let Some(ty) = self.types.facts.expr_types.get(&callee).copied() {
            match self.types.store.get(ty) {
                Some(Type::Nominal(_)) => return true,
                Some(Type::Applied { constructor, .. }) => {
                    if matches!(
                        self.types.store.get(TypeId(constructor.0)),
                        Some(Type::Nominal(_))
                    ) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        matches!(
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol),
            Some(
                SymbolTypeFact::Type { .. }
                    | SymbolTypeFact::NominalType { .. }
                    | SymbolTypeFact::TypeAlias { .. }
            )
        )
    }
}

impl HirAnalysisSemantics for EffectSemantics<'_> {
    type Domain = EffectState;

    fn hir(&self) -> &HirProgram {
        self.hir
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        let anchor = state
            .unit
            .map(DiagnosticAnchor::Unit)
            .unwrap_or(DiagnosticAnchor::Project);
        self.incomplete_at_anchor(anchor, "effect analysis is missing checked facts", state)
    }

    fn after_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        let mut state = state;
        if matches!(self.hir.exprs.get(expr), Some(HirExpr::Index { .. })) {
            let span = self.hir.exprs[expr].span(&self.hir.blocks);
            match self.types.facts.checked_index_errors.get(&expr).copied() {
                Some(error) => state.summary.record_escaping_effect(Effect::Error(error)),
                None if matches!(
                    self.types.facts.index_facts.get(&expr),
                    Some(etas_types::CheckedIndexKind::MapLookup { .. })
                ) => {}
                None => {
                    state = self.incomplete_at(
                        span,
                        "checked index expression requires a materialized IndexError fact",
                        state,
                    );
                }
            }
        }
        if let Some(EffectUnit::Item(item)) = state.unit {
            if matches!(
                self.hir.items.get(item),
                Some(etas_hir::HirItem::TopLevelLet(value)) if value.value == expr
            ) && self.flow_container_missing_latent_sources(&state, expr)
            {
                let span = self.hir.exprs[expr].span(&self.hir.blocks);
                state = self.reject_effect_state(
                    span,
                    "top-level flow-value container requires checked latent source facts",
                    state,
                );
            }
        }
        self.inputs.expr_effects.insert(expr, state.summary.clone());
        state
    }

    fn begin_handler_arm(
        &mut self,
        _arm: HirHandlerArmId,
        mut state: Self::Domain,
    ) -> Self::Domain {
        state.handler_depth += 1;
        state
    }

    fn after_stmt(&mut self, stmt: HirStmtId, mut state: Self::Domain) -> Self::Domain {
        let Some(stmt_data) = self.hir.stmts.get(stmt).cloned() else {
            return state;
        };
        match stmt_data {
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                if let Some(sources) = self.latent_sources_for_expr(&state, value) {
                    self.record_latent_sources_for_pat(&mut state, pat, sources);
                } else if self.flow_container_missing_latent_sources(&state, value) {
                    let span = self.hir.exprs[value].span(&self.hir.blocks);
                    state = self.reject_effect_state(
                        span,
                        "local flow-value container requires checked latent source facts",
                        state,
                    );
                }
            }
            HirStmt::Resume { span, .. } => {
                if state.handler_depth == 0 {
                    self.diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::ResumeOutsideHandler,
                        span,
                        "resume is only valid inside a handler arm",
                    ));
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler);
                } else {
                    state.resume_count += 1;
                }
            }
            HirStmt::Finish { span, .. } => {
                if state.handler_depth == 0 {
                    let _ = span;
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler);
                }
            }
            HirStmt::While { limits, span, .. } | HirStmt::Retry { limits, span, .. } => {
                if limits.is_empty() {
                    self.diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::MissingEffectLoopLimit,
                        span,
                        "effectful loops and retry blocks require an explicit limit",
                    ));
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::MissingRequirement);
                    return state;
                }
                let mut parsed = RequirementSet::new();
                for limit in limits {
                    let Some(requirement) = self.limit_requirement_from_expr(limit) else {
                        self.diagnostics.push(Diagnostic::effect_check(
                            EffectDiagnosticCode::MissingOrInvalidLimit,
                            span,
                            "loop limit requires a checked standard limit constructor",
                        ));
                        state.summary.support = InterpreterSupport::Rejected(
                            FrontendRejectionReason::MissingRequirement,
                        );
                        return state;
                    };
                    parsed.insert(RequirementFact::Limit(requirement));
                }
                state.summary.requirements.union_assign(&parsed);
            }
            _ => {}
        }
        self.inputs.stmt_effects.insert(stmt, state.summary.clone());
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
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.perform_action(expr, action, generic_args, args, span, state))
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
        AnalysisStep::handled(self.std_method_call(expr, span, state))
    }

    fn stage_compose(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if matches!(
            state.unit,
            Some(EffectUnit::AnonymousFlow { value, .. }) if value == expr
        ) {
            return AnalysisStep::handled(self.apply_stage_effects(expr, span, state));
        }
        let Some(owner) = state.owner else {
            state.mark_incomplete();
            return AnalysisStep::handled(state);
        };
        state.record_anonymous_flow(owner, expr, EffectAnonymousFlowBody::Expr(expr));
        AnalysisStep::handled(state)
    }

    fn pipeline(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.apply_stage_effects(expr, span, state))
    }

    fn try_expr(
        &mut self,
        expr: HirExprId,
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if let Some(fact) = self.types.facts.try_facts.get(&expr) {
            let escaping_errors = state
                .summary
                .escaping_effects
                .effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Error(error) => Some(*error),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let Some((captured_error, escaping_error)) =
                self.try_capture_error(fact.target_error, &escaping_errors)
            else {
                let message = if escaping_errors.len() > 1 {
                    "try expression cannot capture from an operand with multiple Error[E] effects; use an explicit handler to select or translate the error"
                } else {
                    "try expression requires a checked captured Error[E] effect"
                };
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::InvalidTryCapture,
                    self.hir.exprs[expr].span(&self.hir.blocks),
                    message,
                ));
                state.mark_incomplete();
                return AnalysisStep::handled(state);
            };
            self.inputs.try_captures.insert(
                expr,
                crate::TryCaptureFact {
                    operand: fact.operand,
                    captured_error,
                    result_type: fact.result_type,
                    conversions: Vec::new(),
                },
            );
            state
                .summary
                .escaping_effects
                .remove_effect(&Effect::Error(escaping_error));
        } else {
            state = self.incomplete_at(
                self.hir.exprs[expr].span(&self.hir.blocks),
                "try expression requires checked try-capture facts",
                state,
            );
        }
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
        span: Span,
        _entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> etas_hir_analysis::intraprocedural::Control<Self::Domain> {
        self.handle_parts(expr, span, parts)
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let Some(owner) = state.owner else {
            state.mark_incomplete();
            return AnalysisStep::handled(state);
        };
        let body = match self.hir.exprs.get(expr) {
            Some(HirExpr::Lambda { body, .. }) => match body {
                etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
                etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
            },
            _ => {
                state.mark_incomplete();
                return AnalysisStep::handled(state);
            }
        };
        state.record_anonymous_flow(owner, expr, body);
        AnalysisStep::handled(state)
    }

    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let Some(HirExpr::Handler { handlers, .. }) = self.hir.exprs.get(expr) else {
            return AnalysisStep::handled(self.incomplete_at(
                span,
                "handler boundary requires a handler expression",
                state,
            ));
        };
        AnalysisStep::handled(self.materialize_handler_value(expr, span, handlers, state))
    }
}

impl InterproceduralSemantics for EffectSemantics<'_> {
    type Unit = EffectUnit;
    type Domain = EffectState;
    type Summary = EffectSummary;

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody {
        unit.analysis_body_with_context(self.hir, &self.context)
    }

    fn begin_unit(&mut self, context: UnitContext<Self::Unit>) -> Self::Domain {
        let mut state = EffectState::for_unit(context.unit);
        if matches!(context.unit, EffectUnit::HandlerArm { .. }) {
            state.handler_depth = 1;
        }
        state
    }

    fn end_unit(
        &mut self,
        context: UnitContext<Self::Unit>,
        exit: etas_hir_analysis::intraprocedural::Control<Self::Domain>,
    ) -> Self::Summary {
        let state = exit.into_joined_domain();
        let mut summary = state.summary;
        if let Some((span, message)) = self.missing_top_level_flow_value_fact(context.unit) {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                message,
            ));
            summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
        }
        self.inputs
            .unit_effects
            .insert(context.unit, summary.clone());
        summary
    }

    fn stabilize_summary(
        &mut self,
        _context: UnitContext<Self::Unit>,
        mut summary: Self::Summary,
        current: Option<&Self::Summary>,
    ) -> Self::Summary {
        summary.stabilize_for_fixpoint(current);
        summary
    }

    fn external_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        match context.unit {
            EffectUnit::Item(item) => {
                let span = self.hir.items[item].span();
                if matches!(
                    self.hir.items.get(item),
                    Some(etas_hir::HirItem::Tool(etas_hir::HirToolDecl {
                        body: etas_hir::HirToolBody::Decl { .. },
                        ..
                    }))
                ) {
                    if self.has_tool_binding(item) {
                        return self
                            .summary_for_item_signature(item, span)
                            .unwrap_or_else(|| {
                                self.incomplete_summary(
                                    span,
                                    "bound bodyless tool requires checked effect metadata",
                                )
                            });
                    }
                    let mut summary = EffectSummary::local();
                    summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
                    return summary;
                }
                self.summary_for_item_signature(item, span)
                    .unwrap_or_else(|| {
                        self.incomplete_summary(
                            span,
                            "external item requires checked effect metadata",
                        )
                    })
            }
            _ => self.incomplete_summary_at_anchor(
                DiagnosticAnchor::Unit(context.unit),
                "external effect unit is not materialized",
            ),
        }
    }

    fn missing_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        self.incomplete_summary_at_anchor(
            DiagnosticAnchor::Unit(context.unit),
            "effect unit body is missing",
        )
    }

    fn call_target(
        &mut self,
        context: UnitContext<Self::Unit>,
        call: HirExprId,
        callee: HirExprId,
        state: &Self::Domain,
    ) -> CallTarget<Self::Unit> {
        self.call_target_for(context, call, callee, state)
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

    fn anonymous_flow_dependency(
        &mut self,
        context: UnitContext<Self::Unit>,
        expr: HirExprId,
    ) -> Option<Self::Unit> {
        self.anonymous_flow_unit_for_expr(context.unit.owner()?, expr)
    }

    fn direct_call(
        &mut self,
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        callee: Self::Unit,
        summary: &Self::Summary,
        mut state: Self::Domain,
    ) -> Self::Domain {
        let summary = self.specialize_summary_for_call(callee, &site, &state, summary);
        state.summary.seq_assign(&summary);
        state = self.apply_deferred_first_class_call_sites(context, site, callee, state);
        self.record_latent_realization(callee, site.call);
        state
    }

    fn dynamic_call(
        &mut self,
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        mut state: Self::Domain,
    ) -> Self::Domain {
        if self.is_nominal_constructor_callee(site.callee_expr)
            || self.pure_type_constructor_call(site.callee_expr)
        {
            return state;
        }
        if let Some(sources) = self.latent_sources_for_expr(&state, site.callee_expr) {
            let mut realized = false;
            for source in sources {
                let Some(summary) = self.inputs.unit_effects.get(&source).cloned() else {
                    continue;
                };
                state.summary.seq_assign(&summary);
                self.record_latent_realization(source, site.call);
                realized = true;
            }
            if realized {
                return state;
            }
        }
        if let Some(summary) = self.summary_for_flow_expr_type(site.callee_expr, site.span) {
            state.summary.seq_assign(&summary);
            return state;
        }
        if let Some(flow) = self.flow_type_for_expr(site.callee_expr) {
            if flow.effects.is_none() && self.is_deferred_first_class_param(site.callee_expr) {
                self.inputs
                    .deferred_first_class_calls
                    .entry(context.unit)
                    .or_default()
                    .push(site.call);
                return state;
            }
            if flow.effects.is_none() && self.is_projected_flow_value(site.callee_expr) {
                return self.reject_effect_state(
                    site.span,
                    "latent flow realization requires checked latent source facts",
                    state,
                );
            }
            return self.incomplete_at(
                site.span,
                "first-class flow call requires a checked latent effect fact or an explicit function effect row",
                state,
            );
        }
        self.incomplete_at(
            site.span,
            "dynamic flow call requires materialized latent effect facts",
            state,
        )
    }

    fn external_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.apply_external_call(site, state)
    }

    fn incomplete_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        if self.is_nominal_constructor_callee(site.callee_expr)
            || self.pure_type_constructor_call(site.callee_expr)
        {
            return state;
        }
        if self.std_pure_support_call(site.callee_expr) {
            return state;
        }
        if let Some(state) = self.partially_resolved_std_method_call(site, state.clone()) {
            return state;
        }
        if self.source_import_target_missing(site.callee_expr) {
            return self.reject_effect_state(
                site.span,
                "source import target requires checked effect facts",
                state,
            );
        }
        if !self.types.facts.expr_types.contains_key(&site.callee_expr) {
            return self.reject_effect_state(
                site.span,
                "first-class flow call requires a checked callee function type",
                state,
            );
        }
        self.reject_effect_state(
            site.span,
            "call target requires checked effect facts",
            state,
        )
    }
}

impl EffectSemantics<'_> {
    fn specialize_summary_for_call(
        &self,
        callee: EffectUnit,
        site: &CallSite<EffectUnit>,
        state: &EffectState,
        summary: &EffectSummary,
    ) -> EffectSummary {
        let Some(params) = self.unit_params(callee) else {
            return summary.clone();
        };
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return summary.clone();
        };
        let mut bindings = Vec::new();
        for (index, param) in params.iter().enumerate() {
            let Some(arg) = args.get(index).and_then(arg_expr) else {
                continue;
            };
            let Some(symbol) = self.hir.symbols.get(*param) else {
                continue;
            };
            bindings.push((symbol.name.clone(), arg));
        }
        let type_bindings =
            self.call_type_bindings(callee, site, state.owner.map(EffectUnit::Item));
        if bindings.is_empty() && type_bindings.is_empty() {
            return summary.clone();
        }

        self.specialize_summary_with_bindings(summary, &bindings, &type_bindings)
    }

    fn call_bindings_from_param_names(
        &self,
        site: &CallSite<EffectUnit>,
        param_names: &[String],
    ) -> Vec<(String, HirExprId)> {
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return Vec::new();
        };
        param_names
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                args.get(index)
                    .and_then(arg_expr)
                    .map(|arg| (param.clone(), arg))
            })
            .collect()
    }

    fn call_type_bindings(
        &self,
        callee: EffectUnit,
        site: &CallSite<EffectUnit>,
        caller: Option<EffectUnit>,
    ) -> Vec<(String, TypeId)> {
        let type_params = self.unit_type_params(callee);
        if type_params.is_empty() {
            return Vec::new();
        }
        let Some(HirExpr::Call {
            generic_args, args, ..
        }) = self.hir.exprs.get(site.call)
        else {
            return Vec::new();
        };

        let mut bindings = Vec::new();
        let type_param_names = type_params
            .iter()
            .filter_map(|symbol| {
                let symbol = self.hir.symbols.get(*symbol)?;
                matches!(symbol.def, SymbolDef::TypeParam { .. }).then(|| symbol.name.clone())
            })
            .collect::<Vec<_>>();

        for (param_name, generic_arg) in type_param_names.iter().zip(generic_args.iter()) {
            let HirGenericArg::Type(ty) = generic_arg else {
                continue;
            };
            if let Some(ty) = self.types.facts.type_refs.get(ty).copied() {
                insert_type_binding(&mut bindings, param_name.clone(), ty);
            }
        }

        if let Some(param_types) = self.unit_param_types(callee) {
            for (param_ty, arg) in param_types.iter().copied().zip(args.iter()) {
                let Some(arg_expr) = arg_expr(arg) else {
                    continue;
                };
                let Some(actual_ty) = self.types.facts.expr_types.get(&arg_expr).copied() else {
                    continue;
                };
                collect_type_bindings_from_type_pattern(
                    &self.types.store,
                    param_ty,
                    actual_ty,
                    &type_param_names,
                    &mut bindings,
                );
            }
        }

        self.infer_type_bindings_from_spec_bounds(&type_params, caller, &mut bindings);
        bindings
    }

    fn unit_type_params(&self, unit: EffectUnit) -> Vec<etas_hir::SymbolId> {
        let EffectUnit::Item(item) = unit else {
            return Vec::new();
        };
        match self.hir.items.get(item) {
            Some(etas_hir::HirItem::Flow(flow)) => flow.type_params.clone(),
            Some(etas_hir::HirItem::Tool(tool)) => tool.type_params.clone(),
            Some(etas_hir::HirItem::Type(decl)) => decl.type_params.clone(),
            Some(etas_hir::HirItem::TypeAlias(decl)) => decl.type_params.clone(),
            Some(etas_hir::HirItem::Enum(decl)) => decl.type_params.clone(),
            _ => Vec::new(),
        }
    }

    fn unit_param_types(&self, unit: EffectUnit) -> Option<Vec<TypeId>> {
        let EffectUnit::Item(item) = unit else {
            return None;
        };
        match self.types.facts.item_signatures.get(&item)? {
            ItemSignature::Flow(signature)
            | ItemSignature::Agent(signature)
            | ItemSignature::Tool(signature) => Some(signature.params.clone()),
            ItemSignature::TopLevelLet(_) => None,
        }
    }

    fn infer_type_bindings_from_spec_bounds(
        &self,
        callee_type_params: &[etas_hir::SymbolId],
        caller: Option<EffectUnit>,
        bindings: &mut Vec<(String, TypeId)>,
    ) {
        let type_param_names = callee_type_params
            .iter()
            .filter_map(|symbol| {
                let symbol = self.hir.symbols.get(*symbol)?;
                matches!(symbol.def, SymbolDef::TypeParam { .. }).then(|| symbol.name.clone())
            })
            .collect::<Vec<_>>();
        let caller_type_param_by_name = caller
            .map(|unit| self.unit_type_params(unit))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|symbol| {
                let symbol_data = self.hir.symbols.get(symbol)?;
                matches!(symbol_data.def, SymbolDef::TypeParam { .. })
                    .then(|| (symbol_data.name.clone(), symbol))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut changed = true;
        while changed {
            changed = false;
            let snapshot = bindings.clone();
            for callee_type_param in callee_type_params {
                let Some(bounds) = self.types.facts.type_param_bounds.get(callee_type_param) else {
                    continue;
                };
                for bound in bounds {
                    let Some(actual_ty) = snapshot
                        .iter()
                        .find(|(name, _)| name == &bound.param_name)
                        .map(|(_, ty)| *ty)
                    else {
                        continue;
                    };
                    let Some(actual_name) = named_type_name(&self.types.store, actual_ty) else {
                        continue;
                    };
                    let Some(actual_param) = caller_type_param_by_name.get(&actual_name) else {
                        continue;
                    };
                    let Some(actual_bounds) = self.types.facts.type_param_bounds.get(actual_param)
                    else {
                        continue;
                    };
                    for actual_bound in actual_bounds.iter().filter(|candidate| {
                        candidate.spec_symbol == bound.spec_symbol
                            && candidate.args.len() == bound.args.len()
                    }) {
                        for (pattern, actual) in bound
                            .args
                            .iter()
                            .copied()
                            .zip(actual_bound.args.iter().copied())
                        {
                            if collect_type_bindings_from_type_pattern(
                                &self.types.store,
                                pattern,
                                actual,
                                &type_param_names,
                                bindings,
                            ) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    fn specialize_summary_with_bindings(
        &self,
        summary: &EffectSummary,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> EffectSummary {
        if bindings.is_empty() && type_bindings.is_empty() {
            return summary.clone();
        }
        let mut specialized = summary.clone();
        specialized.escaping_effects =
            self.specialize_effect_row(&summary.escaping_effects, &bindings, type_bindings);
        specialized.requested_actions =
            self.specialize_effect_row(&summary.requested_actions, &bindings, type_bindings);
        specialized.default_actions =
            self.specialize_effect_row(&summary.default_actions, &bindings, type_bindings);
        specialized.handled_actions =
            self.specialize_effect_row(&summary.handled_actions, &bindings, type_bindings);
        specialized.action_trace =
            self.specialize_action_trace(&summary.action_trace, &bindings, type_bindings);
        specialized
    }

    fn unit_params(&self, unit: EffectUnit) -> Option<Vec<etas_hir::SymbolId>> {
        let EffectUnit::Item(item) = unit else {
            return None;
        };
        match self.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => Some(flow.params.clone()),
            etas_hir::HirItem::Agent(agent) => Some(agent.params.clone()),
            etas_hir::HirItem::Tool(tool) => Some(tool.params.clone()),
            _ => None,
        }
    }

    fn specialize_effect_row(
        &self,
        row: &EffectRow,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> EffectRow {
        EffectRow {
            effects: EffectSet::from_iter(
                row.effects
                    .iter()
                    .cloned()
                    .map(|effect| self.specialize_effect(effect, bindings, type_bindings)),
            ),
            open: row.open,
        }
    }

    fn specialize_effect(
        &self,
        effect: Effect,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> Effect {
        match effect {
            Effect::AppliedAction(mut action) => {
                action.args = action
                    .args
                    .into_iter()
                    .map(|arg| self.specialize_effect_arg(arg, bindings, type_bindings))
                    .collect();
                Effect::AppliedAction(action)
            }
            other => other,
        }
    }

    fn specialize_action_trace(
        &self,
        trace: &ActionTraceDomain,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> ActionTraceDomain {
        match trace {
            ActionTraceDomain::Empty => ActionTraceDomain::Empty,
            ActionTraceDomain::Event(event) => {
                let mut event = event.clone();
                event.action = self.specialize_effect(event.action, bindings, type_bindings);
                ActionTraceDomain::Event(event)
            }
            ActionTraceDomain::Seq(items) => ActionTraceDomain::Seq(
                items
                    .iter()
                    .map(|item| self.specialize_action_trace(item, bindings, type_bindings))
                    .collect(),
            ),
            ActionTraceDomain::Choice(items) => ActionTraceDomain::Choice(
                items
                    .iter()
                    .map(|item| self.specialize_action_trace(item, bindings, type_bindings))
                    .collect(),
            ),
            ActionTraceDomain::Repeat(item) => ActionTraceDomain::Repeat(Box::new(
                self.specialize_action_trace(item, bindings, type_bindings),
            )),
            ActionTraceDomain::UnknownOrder(actions) => {
                ActionTraceDomain::UnknownOrder(EffectSet::from_iter(
                    actions
                        .iter()
                        .cloned()
                        .map(|effect| self.specialize_effect(effect, bindings, type_bindings)),
                ))
            }
        }
    }

    fn specialize_effect_arg(
        &self,
        arg: EffectArgRef,
        bindings: &[(String, HirExprId)],
        type_bindings: &[(String, TypeId)],
    ) -> EffectArgRef {
        if let EffectArgRef::Type(ty) = arg {
            return self
                .specialize_type_effect_arg(ty, type_bindings)
                .map(EffectArgRef::Type)
                .unwrap_or(EffectArgRef::Type(ty));
        }
        let EffectArgRef::Path(path) = &arg else {
            return arg;
        };
        let Some((head, tail)) = path.split_first() else {
            return arg;
        };
        let Some((_, expr)) = bindings.iter().find(|(param, _)| param == head) else {
            return self
                .specialize_url_host_projection_arg(path, bindings)
                .unwrap_or(arg);
        };
        self.effect_arg_from_expr_path(*expr, tail)
            .or_else(|| self.specialize_url_host_projection_arg(path, bindings))
            .unwrap_or(arg)
    }

    fn specialize_type_effect_arg(
        &self,
        ty: TypeId,
        type_bindings: &[(String, TypeId)],
    ) -> Option<TypeId> {
        let Type::Named(name) = self.types.store.get(ty)? else {
            return None;
        };
        type_bindings
            .iter()
            .find(|(param, _)| param == &name.name)
            .map(|(_, ty)| *ty)
    }

    fn specialize_url_host_projection_arg(
        &self,
        path: &[String],
        bindings: &[(String, HirExprId)],
    ) -> Option<EffectArgRef> {
        if path.len() < 2 || path[path.len() - 2] != "url" || path[path.len() - 1] != "host" {
            return None;
        }
        let (_, url_expr) = bindings.iter().find(|(param, _)| param == "url")?;
        let value = self.string_arg_from_expr_path_with_bindings(
            *url_expr,
            &[],
            bindings,
            EFFECT_ARG_EVAL_DEPTH_LIMIT,
        )?;
        host_from_absolute_url(&value).map(EffectArgRef::String)
    }

    pub(crate) fn effect_arg_from_expr_path(
        &self,
        expr: HirExprId,
        path: &[String],
    ) -> Option<EffectArgRef> {
        self.effect_arg_from_expr_path_with_bindings(expr, path, &[], EFFECT_ARG_EVAL_DEPTH_LIMIT)
    }

    fn effect_arg_from_expr_path_with_bindings(
        &self,
        expr: HirExprId,
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<EffectArgRef> {
        if let Some(value) =
            self.string_arg_from_expr_path_with_bindings(expr, path, bindings, depth)
        {
            return Some(EffectArgRef::String(value));
        }
        if depth == 0 {
            return None;
        }
        match self.hir.exprs.get(expr)? {
            HirExpr::Record(record) => {
                let (field_name, rest) = path.split_first()?;
                for field in &record.fields {
                    match field {
                        etas_hir::HirFieldInit::Named { name, value, .. } if name == field_name => {
                            return self.effect_arg_from_expr_path_with_bindings(
                                *value,
                                rest,
                                bindings,
                                depth - 1,
                            );
                        }
                        etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                            let ResolveResult::Resolved(symbol) = resolution else {
                                continue;
                            };
                            let symbol_id = *symbol;
                            let Some(symbol) = self.hir.symbols.get(symbol_id) else {
                                continue;
                            };
                            if &symbol.name == field_name {
                                return self.effect_arg_from_symbol(
                                    symbol_id,
                                    rest,
                                    bindings,
                                    depth - 1,
                                );
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            HirExpr::Field { base, field, .. } => {
                let mut combined = Vec::with_capacity(path.len() + 1);
                combined.push(field.clone());
                combined.extend(path.iter().cloned());
                self.effect_arg_from_expr_path_with_bindings(*base, &combined, bindings, depth - 1)
            }
            HirExpr::Path(path_expr) => {
                let (symbol, path) = self.resolve_path_symbol_and_projection(path_expr, path)?;
                self.effect_arg_from_symbol(symbol, &path, bindings, depth - 1)
            }
            HirExpr::Call { callee, args, .. } => {
                self.effect_arg_from_call_path(*callee, args, path, bindings, depth - 1)
            }
            _ => None,
        }
    }

    fn string_arg_from_expr_path_with_bindings(
        &self,
        expr: HirExprId,
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<String> {
        if depth == 0 {
            return None;
        }
        match self.hir.exprs.get(expr)? {
            HirExpr::Literal(HirLiteral::String { value, .. }) if path.is_empty() => {
                Some(value.clone())
            }
            HirExpr::Record(record) => {
                let (field_name, rest) = path.split_first()?;
                for field in &record.fields {
                    match field {
                        etas_hir::HirFieldInit::Named { name, value, .. } if name == field_name => {
                            return self.string_arg_from_expr_path_with_bindings(
                                *value,
                                rest,
                                bindings,
                                depth - 1,
                            );
                        }
                        etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                            let ResolveResult::Resolved(symbol) = resolution else {
                                continue;
                            };
                            let symbol_id = *symbol;
                            let Some(symbol) = self.hir.symbols.get(symbol_id) else {
                                continue;
                            };
                            if &symbol.name == field_name {
                                return self.string_arg_from_symbol(
                                    symbol_id,
                                    rest,
                                    bindings,
                                    depth - 1,
                                );
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            HirExpr::Field { base, field, .. } => {
                let mut combined = Vec::with_capacity(path.len() + 1);
                combined.push(field.clone());
                combined.extend(path.iter().cloned());
                self.string_arg_from_expr_path_with_bindings(*base, &combined, bindings, depth - 1)
            }
            HirExpr::Path(path_expr) => {
                let (symbol, path) = self.resolve_path_symbol_and_projection(path_expr, path)?;
                self.string_arg_from_symbol(symbol, &path, bindings, depth - 1)
            }
            HirExpr::Call { callee, args, .. } => {
                self.string_arg_from_call_path(*callee, args, path, bindings, depth - 1)
            }
            _ => None,
        }
    }

    fn resolve_path_symbol_and_projection(
        &self,
        path_expr: &etas_hir::ResolvedPath,
        path: &[String],
    ) -> Option<(etas_hir::SymbolId, Vec<String>)> {
        match &path_expr.resolution {
            ResolveResult::Resolved(symbol) => Some((*symbol, path.to_vec())),
            ResolveResult::PartiallyResolved(partial)
                if partial.reason
                    == etas_hir::PartialResolutionReason::MemberRequiresTypeChecking =>
            {
                let mut combined = partial.remaining.clone();
                combined.extend(path.iter().cloned());
                Some((partial.resolved_prefix?, combined))
            }
            _ => None,
        }
    }

    fn string_arg_from_symbol(
        &self,
        symbol: etas_hir::SymbolId,
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<String> {
        let symbol = self.hir.symbols.get(symbol)?;
        if let Some((_, expr)) = bindings.iter().find(|(param, expr)| {
            param.as_str() == symbol.name.as_str()
                && !self.expr_resolves_to_symbol(*expr, symbol.id)
        }) {
            return self.string_arg_from_expr_path_with_bindings(*expr, path, bindings, depth);
        }
        match symbol.def {
            SymbolDef::Local {
                initializer: Some(initializer),
                ..
            } => self.string_arg_from_expr_path_with_bindings(initializer, path, bindings, depth),
            _ => None,
        }
    }

    fn effect_arg_from_symbol(
        &self,
        symbol: etas_hir::SymbolId,
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<EffectArgRef> {
        let symbol_data = self.hir.symbols.get(symbol)?;
        if let Some((_, expr)) = bindings.iter().find(|(param, expr)| {
            param.as_str() == symbol_data.name.as_str()
                && !self.expr_resolves_to_symbol(*expr, symbol)
        }) {
            return self.effect_arg_from_expr_path_with_bindings(*expr, path, bindings, depth);
        }
        match symbol_data.def {
            SymbolDef::Local {
                initializer: Some(initializer),
                ..
            }
            | SymbolDef::PatternBinding {
                initializer: Some(initializer),
                ..
            } => self.effect_arg_from_expr_path_with_bindings(initializer, path, bindings, depth),
            SymbolDef::Param { .. } => {
                let mut segments = Vec::with_capacity(path.len() + 1);
                segments.push(symbol_data.name.clone());
                segments.extend(path.iter().cloned());
                Some(EffectArgRef::Path(segments))
            }
            _ => None,
        }
    }

    fn expr_resolves_to_symbol(&self, expr: HirExprId, target: etas_hir::SymbolId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) else {
            return false;
        };
        match &path.resolution {
            ResolveResult::Resolved(symbol) => *symbol == target,
            ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix == Some(target),
            _ => false,
        }
    }

    fn string_arg_from_call_path(
        &self,
        callee: HirExprId,
        args: &[HirArg],
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<String> {
        if let Some(value) = self.string_arg_from_std_text_call(callee, args, path, bindings, depth)
        {
            return Some(value);
        }
        if let Some(value) =
            self.string_arg_from_source_flow_call(callee, args, path, bindings, depth)
        {
            return Some(value);
        }
        self.string_arg_from_url_literal_projection(callee, args, path, bindings, depth)
    }

    fn effect_arg_from_call_path(
        &self,
        callee: HirExprId,
        args: &[HirArg],
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<EffectArgRef> {
        if let Some(value) = self.string_arg_from_call_path(callee, args, path, bindings, depth) {
            return Some(EffectArgRef::String(value));
        }
        if self.is_std_text_value_transform(callee) {
            return args.first().and_then(arg_expr).and_then(|arg| {
                self.effect_arg_from_expr_path_with_bindings(arg, &[], bindings, depth)
            });
        }
        let item = self.source_flow_item_for_callee(callee)?;
        let etas_hir::HirItem::Flow(flow) = self.hir.items.get(item)? else {
            return None;
        };
        let return_expr = self.single_return_expr_for_flow(flow)?;
        let mut call_bindings = self.flow_call_bindings(flow, args);
        call_bindings.extend_from_slice(bindings);
        self.effect_arg_from_expr_path_with_bindings(return_expr, path, &call_bindings, depth)
    }

    fn string_arg_from_std_text_call(
        &self,
        callee: HirExprId,
        args: &[HirArg],
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<String> {
        if !path.is_empty() {
            return None;
        }
        let HirExpr::Path(path_expr) = self.hir.exprs.get(callee)? else {
            return None;
        };
        let segments = self.canonical_static_resource_path_segments(path_expr)?;
        if segments.len() != 3 || segments[0] != "std" || segments[1] != "text" {
            return None;
        }
        let input = args.first().and_then(arg_expr).and_then(|arg| {
            self.string_arg_from_expr_path_with_bindings(arg, &[], bindings, depth)
        })?;
        match segments[2].as_str() {
            "trim" => Some(input.trim().to_owned()),
            "uppercase" => Some(input.to_uppercase()),
            "lowercase" => Some(input.to_lowercase()),
            _ => None,
        }
    }

    fn string_arg_from_source_flow_call(
        &self,
        callee: HirExprId,
        args: &[HirArg],
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<String> {
        let item = self.source_flow_item_for_callee(callee)?;
        let etas_hir::HirItem::Flow(flow) = self.hir.items.get(item)? else {
            return None;
        };
        let return_expr = self.single_return_expr_for_flow(flow)?;
        let mut call_bindings = self.flow_call_bindings(flow, args);
        call_bindings.extend_from_slice(bindings);
        self.string_arg_from_expr_path_with_bindings(return_expr, path, &call_bindings, depth)
    }

    fn flow_call_bindings(
        &self,
        flow: &etas_hir::HirFlowDecl,
        args: &[HirArg],
    ) -> Vec<(String, HirExprId)> {
        let mut call_bindings = Vec::new();
        for (index, param) in flow.params.iter().enumerate() {
            let Some(arg) = args.get(index).and_then(arg_expr) else {
                continue;
            };
            let Some(symbol) = self.hir.symbols.get(*param) else {
                continue;
            };
            call_bindings.push((symbol.name.clone(), arg));
        }
        call_bindings
    }

    fn source_flow_item_for_callee(&self, callee: HirExprId) -> Option<HirItemId> {
        let HirExpr::Path(path) = self.hir.exprs.get(callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        match self.hir.symbols.get(symbol)?.def {
            SymbolDef::Item { item } => {
                matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Flow(_))).then_some(item)
            }
            SymbolDef::ImportAlias {
                ref path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => self.source_item_for_path(path).filter(|item| {
                matches!(self.hir.items.get(*item), Some(etas_hir::HirItem::Flow(_)))
            }),
            _ => None,
        }
    }

    fn single_return_expr_for_flow(&self, flow: &etas_hir::HirFlowDecl) -> Option<HirExprId> {
        match flow.body {
            etas_hir::HirFlowBody::Expr { expr, .. } => Some(expr),
            etas_hir::HirFlowBody::Block(block) => self.single_return_expr_for_block(block),
        }
    }

    fn single_return_expr_for_block(&self, block: etas_hir::HirBlockId) -> Option<HirExprId> {
        let block = self.hir.blocks.get(block)?;
        if !block.stmts.iter().all(|stmt| {
            matches!(
                self.hir.stmts.get(*stmt),
                Some(HirStmt::Let { .. } | HirStmt::Return { .. })
            )
        }) {
            return None;
        }
        block
            .stmts
            .iter()
            .rev()
            .find_map(|stmt| match self.hir.stmts.get(*stmt)? {
                HirStmt::Return {
                    value: Some(value), ..
                } => Some(*value),
                _ => None,
            })
            .or(block.final_expr)
    }

    fn string_arg_from_url_literal_projection(
        &self,
        _callee: HirExprId,
        args: &[HirArg],
        path: &[String],
        bindings: &[(String, HirExprId)],
        depth: usize,
    ) -> Option<String> {
        if !matches!(path, [field] if field == "host")
            && !matches!(path, [record, field] if record == "url" && field == "host")
        {
            return None;
        }
        let value = args.first().and_then(arg_expr).and_then(|arg| {
            self.string_arg_from_expr_path_with_bindings(arg, &[], bindings, depth)
        })?;
        host_from_absolute_url(&value)
    }

    fn is_std_text_value_transform(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path_expr)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let Some(segments) = self.canonical_static_resource_path_segments(path_expr) else {
            return false;
        };
        segments.len() == 3
            && segments[0] == "std"
            && segments[1] == "text"
            && matches!(segments[2].as_str(), "trim" | "uppercase" | "lowercase")
    }

    fn source_import_target_missing(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return false;
        };
        let etas_hir::SymbolDef::ImportAlias {
            path,
            origin: etas_hir::ImportAliasOrigin::SourceImport,
        } = &symbol.def
        else {
            return false;
        };
        !path.first().is_some_and(|segment| segment == "std")
            && self.source_item_for_path(path).is_none()
            && !self.has_external_summary_for_path(path)
    }

    pub(crate) fn has_external_summary_for_path(&self, path: &[String]) -> bool {
        self.external_summaries
            .iter()
            .any(|summary| external_summary_matches_path(summary, path))
    }

    pub(crate) fn external_summary_for_path(
        &self,
        path: &[String],
        span: Span,
    ) -> Option<EffectSummary> {
        let metadata = self
            .external_summaries
            .iter()
            .find(|summary| external_summary_matches_path(summary, path))?;
        self.external_summary_from_metadata(metadata, span)
    }

    pub(crate) fn external_call_summary_for_path(
        &mut self,
        path: &[String],
        site: &CallSite<EffectUnit>,
        state: &EffectState,
    ) -> Option<EffectSummary> {
        let metadata = self
            .external_summaries
            .iter()
            .find(|summary| external_summary_matches_path(summary, path))?
            .clone();
        let mut summary = self.external_summary_from_metadata(&metadata, site.span)?;
        let bindings = self.call_bindings_from_param_names(site, &metadata.param_names);
        summary = self.specialize_summary_with_bindings(&summary, &bindings, &[]);
        self.apply_external_function_parameter_effects(&mut summary, site, state)?;
        Some(summary)
    }

    fn external_summary_from_metadata(
        &self,
        metadata: &crate::ExternalEffectSummaryMetadata,
        span: Span,
    ) -> Option<EffectSummary> {
        let mut summary = EffectSummary::local();
        for effect in &metadata.public_effects.effects {
            let effect = self.external_effect_from_metadata(effect)?;
            self.apply_effect_to_summary(&mut summary, effect, span);
        }
        let handled_effects = metadata
            .handled_requested_actions
            .effects
            .iter()
            .filter_map(|handled| self.external_effect_from_metadata(handled))
            .collect::<BTreeSet<_>>();
        for action in &metadata.requested_actions.effects {
            let effect = self.external_effect_from_metadata(action)?;
            let handled = handled_effects.contains(&effect);
            self.record_external_summary_requested_action(&mut summary, effect, handled, span);
        }
        Some(summary)
    }

    fn external_effect_from_metadata(
        &self,
        metadata: &crate::ExternalEffectMetadata,
    ) -> Option<Effect> {
        let name = metadata.path.join(".");
        if self.is_core_error_effect_name(&name) {
            return match metadata.args.as_slice() {
                [EffectArgRef::Type(error)] => Some(Effect::Error(*error)),
                [EffectArgRef::Path(path)] => self
                    .type_id_for_external_effect_path(path)
                    .map(Effect::Error),
                [] => self.registry.tag_by_name("Error").map(Effect::Tag),
                _ => None,
            };
        }
        if let Some(action) = self.registry.action_by_name(&name) {
            return if metadata.args.is_empty() {
                Some(self.action_effect_with_omitted_selector(action))
            } else {
                Some(Effect::AppliedAction(ActionInstanceRef {
                    action,
                    args: metadata.args.clone(),
                }))
            };
        }
        let tag = self.registry.tag_by_name(&name)?;
        let args = self.external_effect_type_args(&metadata.args)?;
        Some(if args.is_empty() {
            Effect::Tag(tag)
        } else {
            Effect::Applied { tag, args }
        })
    }

    fn external_effect_type_args(&self, args: &[EffectArgRef]) -> Option<Vec<TypeId>> {
        args.iter()
            .map(|arg| match arg {
                EffectArgRef::Type(ty) => Some(*ty),
                EffectArgRef::Path(path) => self.type_id_for_external_effect_path(path),
                _ => None,
            })
            .collect()
    }

    fn record_external_summary_requested_action(
        &self,
        summary: &mut EffectSummary,
        effect: Effect,
        handled: bool,
        span: Span,
    ) {
        if !effect.is_action() {
            self.apply_effect_to_summary(summary, effect, span);
            return;
        }
        match &effect {
            Effect::Action(action) => {
                summary.record_requested_action(effect.clone());
                if handled {
                    summary.record_default_handled_action(effect.clone());
                }
                summary.record_action_trace_event(
                    effect.clone(),
                    span,
                    crate::ActionEventSource::ExternalMetadata,
                );
                if let Some(reason) = self
                    .registry
                    .runtime_requirement_reason_for_action_ref(action)
                {
                    summary.require_runtime(reason);
                }
            }
            Effect::AppliedAction(action) => {
                summary.record_requested_action(effect.clone());
                if handled {
                    summary.record_default_handled_action(effect.clone());
                }
                summary.record_action_trace_event(
                    effect.clone(),
                    span,
                    crate::ActionEventSource::ExternalMetadata,
                );
                if let Some(reason) = self
                    .registry
                    .runtime_requirement_reason_for_action_ref(&action.action)
                {
                    summary.require_runtime(reason);
                }
            }
            _ => {}
        }
    }

    fn apply_external_function_parameter_effects(
        &mut self,
        summary: &mut EffectSummary,
        site: &CallSite<EffectUnit>,
        state: &EffectState,
    ) -> Option<()> {
        let Some(callee_input) = self
            .flow_type_for_expr(site.callee_expr)
            .map(|flow| flow.input.clone())
        else {
            return Some(());
        };
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return Some(());
        };
        for (index, param_ty) in callee_input.iter().enumerate() {
            let Some(arg) = args.get(index).and_then(arg_expr) else {
                continue;
            };
            let Some(Type::Function(flow)) = self.types.store.get(*param_ty) else {
                continue;
            };
            if let Some(row) = &flow.effects {
                self.apply_public_row_to_summary(summary, &self.row_from_type_ref(row), site.span);
                continue;
            }
            let Some(sources) = self.latent_sources_for_expr(state, arg) else {
                return None;
            };
            for source in sources {
                let source_summary = self.inputs.unit_effects.get(&source)?.clone();
                summary.seq_assign(&source_summary);
                self.record_latent_realization(source, site.call);
            }
        }
        Some(())
    }

    fn limit_requirement_from_expr(&self, expr: HirExprId) -> Option<LimitRequirement> {
        let (callee, args) = match self.hir.exprs.get(expr)? {
            HirExpr::Call { callee, args, .. } => (*callee, args.as_slice()),
            _ => return None,
        };
        let HirExpr::Path(path) = self.hir.exprs.get(callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.hir.symbols.get(symbol)?;
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return None;
        };
        let std_registry = etas_std::standard_registry();
        let std_symbol = std_registry.lookup_qualified(path)?;
        let StdDecl::Requirement(requirement) = &std_symbol.decl else {
            return None;
        };
        let RequirementSemantics::Limit(std_kind) = requirement.semantics else {
            return None;
        };
        let kind = limit_kind_from_std(std_kind);
        let value = self.limit_value_from_args(kind, args)?;
        Some(LimitRequirement {
            kind,
            budget: kind.budget_kind(),
            value: Some(value),
        })
    }

    fn limit_value_from_args(&self, kind: LimitKind, args: &[HirArg]) -> Option<LimitValue> {
        if args.len() != 1 {
            return None;
        }
        let expr = match args.first()? {
            HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
        };
        let HirExpr::Literal(HirLiteral::Int { text, .. }) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let value = text.replace('_', "").parse::<u64>().ok()?;
        Some(match kind {
            LimitKind::Iterations
            | LimitKind::Tokens
            | LimitKind::ContextTokens
            | LimitKind::Attempts => LimitValue::Count(value),
            LimitKind::Cost => LimitValue::MoneyMicros {
                amount: u128::from(value),
                currency: "USD".to_owned(),
            },
            LimitKind::WallTime => LimitValue::DurationMillis(value),
        })
    }

    fn apply_stage_effects(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: EffectState,
    ) -> EffectState {
        let stages = match self.hir.exprs.get(expr) {
            Some(HirExpr::StageCompose { stages, .. }) | Some(HirExpr::Pipeline { stages, .. }) => {
                stages.clone()
            }
            _ => Vec::new(),
        };
        for stage in stages {
            let mut parsed_limits = RequirementSet::new();
            for limit in stage.limits {
                let Some(requirement) = self.limit_requirement_from_expr(limit) else {
                    state = self.incomplete_at(
                        span,
                        "pipeline stage limit requires a checked standard limit constructor",
                        state,
                    );
                    continue;
                };
                parsed_limits.insert(RequirementFact::Limit(requirement));
            }
            state.summary.requirements.union_assign(&parsed_limits);

            if let Some(summary) = self.summary_for_callable_expr(stage.expr, span) {
                state.summary.seq_assign(&summary);
            } else if self.is_typed_flow_expr(stage.expr) {
                state = self.incomplete_at(
                    span,
                    "pipeline stage requires materialized callable effect facts",
                    state,
                );
            }
        }
        state
    }

    fn summary_for_callable_expr(&self, expr: HirExprId, span: Span) -> Option<EffectSummary> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return self.summary_for_flow_expr_type(expr, span);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return self.summary_for_flow_expr_type(expr, span);
        };
        let symbol = self.hir.symbols.get(symbol)?;
        match &symbol.def {
            etas_hir::SymbolDef::Item { item } => {
                if matches!(self.hir.items.get(*item), Some(etas_hir::HirItem::Agent(_))) {
                    return self.summary_for_agent_call(*item, span);
                }
                self.inputs
                    .unit_effects
                    .get(&EffectUnit::Item(*item))
                    .cloned()
                    .or_else(|| self.summary_for_item_public_effects(*item, span))
            }
            etas_hir::SymbolDef::ImportAlias {
                path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => self
                .source_item_for_path(path)
                .and_then(|item| {
                    if matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Agent(_))) {
                        self.summary_for_agent_call(item, span)
                    } else {
                        self.summary_for_item_public_effects(item, span)
                    }
                })
                .or_else(|| self.external_summary_for_path(path, span)),
            _ => self.summary_for_flow_expr_type(expr, span),
        }
    }

    pub(crate) fn source_item_for_path(&self, path: &[String]) -> Option<HirItemId> {
        let (name, module_path) = path.split_last()?;
        for (item, hir_item) in self.hir.items.iter() {
            let Some(symbol) = (match hir_item {
                etas_hir::HirItem::Flow(flow) => Some(flow.symbol),
                etas_hir::HirItem::Agent(agent) => Some(agent.symbol),
                etas_hir::HirItem::Tool(tool) => Some(tool.symbol),
                etas_hir::HirItem::TopLevelLet(value) => Some(value.symbol),
                _ => None,
            }) else {
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

    pub(crate) fn agent_infer_summary(&self, item: HirItemId, span: Span) -> Option<EffectSummary> {
        if !matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Agent(_))) {
            return None;
        }
        let action = self
            .registry
            .core_action(CoreEffect::Agentic, AGENTIC_INFER_ACTION)?;
        let path = self.item_effect_path(item)?;
        let mut summary = EffectSummary::local();
        let effect = Effect::AppliedAction(ActionInstanceRef {
            action: action.clone(),
            args: vec![EffectArgRef::Path(path)],
        });
        summary.record_requested_action(effect.clone());
        summary.record_default_handled_action(effect.clone());
        summary.record_action_trace_event(effect, span, crate::ActionEventSource::AgentCall);
        if let Some(reason) = self
            .registry
            .runtime_requirement_reason_for_action_ref(&action)
        {
            summary.require_runtime(reason);
        }
        Some(summary)
    }

    fn item_effect_path(&self, item: HirItemId) -> Option<Vec<String>> {
        let symbol = match self.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => flow.symbol,
            etas_hir::HirItem::Agent(agent) => agent.symbol,
            etas_hir::HirItem::Tool(tool) => tool.symbol,
            etas_hir::HirItem::TopLevelLet(value) => value.symbol,
            _ => return None,
        };
        let symbol = self.hir.symbols.get(symbol)?;
        let mut path = self
            .hir
            .modules_arena
            .get(symbol.defining_module)?
            .name
            .as_ref()
            .map(|module| {
                module
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        path.push(symbol.name.clone());
        Some(path)
    }

    fn incomplete_summary(&mut self, span: Span, message: &'static str) -> EffectSummary {
        self.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
        let mut summary = EffectSummary::local();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::UnresolvedEffect);
        summary
    }

    fn missing_top_level_flow_value_fact(&self, unit: EffectUnit) -> Option<(Span, &'static str)> {
        let EffectUnit::Item(item) = unit else {
            return None;
        };
        let etas_hir::HirItem::TopLevelLet(value) = self.hir.items.get(item)? else {
            return None;
        };
        if !matches!(
            self.hir.exprs.get(value.value),
            Some(HirExpr::Lambda { .. })
        ) {
            return None;
        }
        if self.types.facts.symbol_types.contains_key(&value.symbol) {
            return None;
        }
        Some((
            value.span,
            "top-level flow value requires checked value type facts",
        ))
    }

    fn latent_sources_for_expr(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> Option<Vec<EffectUnit>> {
        if let Some(sources) = state.local_latent_values.value_sources(expr) {
            if !sources.is_empty() {
                return Some(sources.iter().copied().collect());
            }
        }
        match self.hir.exprs.get(expr)? {
            HirExpr::Path(path) => {
                let symbol = match &path.resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix?,
                    ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => return None,
                };
                if let Some(sources) = state.local_latent_values.symbol_sources(symbol) {
                    if !sources.is_empty() {
                        return Some(sources.iter().copied().collect());
                    }
                }
                let symbol_data = self.hir.symbols.get(symbol)?;
                match symbol_data.def {
                    SymbolDef::Item { item } => self
                        .top_level_anonymous_flow_unit(item)
                        .map(|unit| vec![unit]),
                    SymbolDef::TopLevelLet { item, .. } => self
                        .top_level_anonymous_flow_unit(item)
                        .map(|unit| vec![unit]),
                    _ => None,
                }
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => self.union_latent_sources(
                elems
                    .iter()
                    .filter_map(|elem| self.latent_sources_for_expr(state, *elem)),
            ),
            HirExpr::Map { entries, .. } => self.union_latent_sources(
                entries
                    .iter()
                    .filter_map(|entry| self.latent_sources_for_expr(state, entry.value)),
            ),
            HirExpr::Record(record) => {
                self.union_latent_sources(record.fields.iter().filter_map(|field| match field {
                    etas_hir::HirFieldInit::Named { value, .. } => {
                        self.latent_sources_for_expr(state, *value)
                    }
                    etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                        let ResolveResult::Resolved(symbol) = resolution else {
                            return None;
                        };
                        state
                            .local_latent_values
                            .symbol_sources(*symbol)
                            .map(|sources| sources.iter().copied().collect())
                    }
                }))
            }
            HirExpr::Index { base, .. } | HirExpr::Field { base, .. } => {
                self.latent_sources_for_expr(state, *base)
            }
            HirExpr::Call { callee, args, .. } if self.is_wrapper_or_unwrap_call(*callee) => args
                .first()
                .and_then(|arg| arg_expr(arg))
                .and_then(|arg| self.latent_sources_for_expr(state, arg)),
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => self
                .union_latent_sources(
                    args.iter()
                        .filter_map(arg_expr)
                        .filter_map(|arg| self.latent_sources_for_expr(state, arg)),
                ),
            HirExpr::StageCompose { .. } => state
                .local_latent_values
                .value_sources(expr)
                .map(|sources| sources.iter().copied().collect()),
            _ => None,
        }
    }

    fn union_latent_sources(
        &self,
        sources: impl IntoIterator<Item = Vec<EffectUnit>>,
    ) -> Option<Vec<EffectUnit>> {
        let mut merged = std::collections::BTreeSet::new();
        for sources in sources {
            merged.extend(sources);
        }
        (!merged.is_empty()).then(|| merged.into_iter().collect())
    }

    fn is_wrapper_or_unwrap_call(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        path.segments
            .last()
            .is_some_and(|segment| matches!(segment.name.as_str(), "Some" | "Ok" | "unwrap"))
    }

    fn is_value_constructor_call(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        self.hir.symbols.get(symbol).is_some_and(|symbol| {
            matches!(
                symbol.kind,
                etas_hir::SymbolKind::Type
                    | etas_hir::SymbolKind::Enum
                    | etas_hir::SymbolKind::EnumVariant
            )
        })
    }

    fn record_latent_sources_for_pat(
        &self,
        state: &mut EffectState,
        pat: HirPatId,
        sources: Vec<EffectUnit>,
    ) {
        let Some(pat_data) = self.hir.pats.get(pat) else {
            return;
        };
        match pat_data {
            HirPat::Binding { symbol, .. } => {
                state
                    .local_latent_values
                    .record_symbol_sources(*symbol, sources);
            }
            HirPat::Tuple { elems, .. } | HirPat::Variant { args: elems, .. } => {
                for elem in elems {
                    self.record_latent_sources_for_pat(state, *elem, sources.clone());
                }
            }
            HirPat::Record { fields, .. } => {
                for field in fields {
                    if let Some(pat) = field.pat {
                        self.record_latent_sources_for_pat(state, pat, sources.clone());
                    }
                }
            }
            HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => {}
        }
    }

    pub(crate) fn top_level_anonymous_flow_unit(&self, item: HirItemId) -> Option<EffectUnit> {
        let etas_hir::HirItem::TopLevelLet(value) = self.hir.items.get(item)? else {
            return None;
        };
        let HirExpr::Lambda { body, .. } = self.hir.exprs.get(value.value)? else {
            return None;
        };
        let body = match body {
            etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
            etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
        };
        Some(EffectUnit::AnonymousFlow {
            owner: item,
            value: value.value,
            body,
        })
    }

    pub(crate) fn local_anonymous_flow_unit_for_symbol(
        &self,
        owner: HirItemId,
        symbol: etas_hir::SymbolId,
    ) -> Option<EffectUnit> {
        let body = self.owner_primary_block(owner)?;
        self.local_anonymous_flow_in_block(owner, body, symbol)
    }

    fn local_anonymous_flow_in_block(
        &self,
        owner: HirItemId,
        block: etas_hir::HirBlockId,
        symbol: etas_hir::SymbolId,
    ) -> Option<EffectUnit> {
        let block = self.hir.blocks.get(block)?;
        for stmt in &block.stmts {
            let Some(stmt) = self.hir.stmts.get(*stmt) else {
                continue;
            };
            match stmt {
                HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. }
                    if self.pat_binds_symbol(*pat, symbol) =>
                {
                    if let Some(unit) = self.anonymous_flow_unit_for_expr(owner, *value) {
                        return Some(unit);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn pat_binds_symbol(&self, pat: HirPatId, symbol: etas_hir::SymbolId) -> bool {
        let Some(pat) = self.hir.pats.get(pat) else {
            return false;
        };
        match pat {
            HirPat::Binding { symbol: bound, .. } => *bound == symbol,
            HirPat::Tuple { elems, .. } | HirPat::Variant { args: elems, .. } => {
                elems.iter().any(|pat| self.pat_binds_symbol(*pat, symbol))
            }
            HirPat::Record { fields, .. } => fields
                .iter()
                .filter_map(|field| field.pat)
                .any(|pat| self.pat_binds_symbol(pat, symbol)),
            HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => false,
        }
    }

    fn anonymous_flow_unit_for_expr(
        &self,
        owner: HirItemId,
        value: HirExprId,
    ) -> Option<EffectUnit> {
        match self.hir.exprs.get(value)? {
            HirExpr::Lambda { body, .. } => {
                let body = match body {
                    etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
                    etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
                };
                Some(EffectUnit::AnonymousFlow { owner, value, body })
            }
            HirExpr::Path(path) => {
                let symbol = match &path.resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix?,
                    ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => return None,
                };
                let symbol = self.hir.symbols.get(symbol)?;
                match symbol.def {
                    SymbolDef::Local {
                        initializer: Some(initializer),
                        ..
                    } => self.anonymous_flow_unit_for_expr(owner, initializer),
                    SymbolDef::TopLevelLet { item, .. } | SymbolDef::Item { item } => {
                        self.top_level_anonymous_flow_unit(item)
                    }
                    _ => None,
                }
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => elems
                .iter()
                .find_map(|expr| self.anonymous_flow_unit_for_expr(owner, *expr)),
            HirExpr::Map { entries, .. } => entries
                .iter()
                .find_map(|entry| self.anonymous_flow_unit_for_expr(owner, entry.value)),
            HirExpr::Record(record) => record.fields.iter().find_map(|field| match field {
                etas_hir::HirFieldInit::Named { value, .. } => {
                    self.anonymous_flow_unit_for_expr(owner, *value)
                }
                etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                    let ResolveResult::Resolved(symbol) = resolution else {
                        return None;
                    };
                    self.hir
                        .symbols
                        .get(*symbol)
                        .and_then(|symbol| match symbol.def {
                            SymbolDef::Local {
                                initializer: Some(initializer),
                                ..
                            } => self.anonymous_flow_unit_for_expr(owner, initializer),
                            _ => None,
                        })
                }
            }),
            HirExpr::Index { base, .. } | HirExpr::Field { base, .. } => {
                self.anonymous_flow_unit_for_expr(owner, *base)
            }
            HirExpr::Call { callee, args, .. } if self.is_wrapper_or_unwrap_call(*callee) => args
                .first()
                .and_then(arg_expr)
                .and_then(|arg| self.anonymous_flow_unit_for_expr(owner, arg)),
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => args
                .iter()
                .filter_map(arg_expr)
                .find_map(|arg| self.anonymous_flow_unit_for_expr(owner, arg)),
            HirExpr::StageCompose { .. } => Some(EffectUnit::AnonymousFlow {
                owner,
                value,
                body: EffectAnonymousFlowBody::Expr(value),
            }),
            _ => None,
        }
    }

    pub(crate) fn reject_effect_state(
        &mut self,
        span: Span,
        message: &'static str,
        mut state: EffectState,
    ) -> EffectState {
        self.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
        state.mark_incomplete();
        state.summary.support =
            InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
        state
    }

    fn flow_container_missing_latent_sources(&self, state: &EffectState, expr: HirExprId) -> bool {
        let Some(expr_data) = self.hir.exprs.get(expr) else {
            return false;
        };
        match expr_data {
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => elems
                .iter()
                .any(|elem| self.flow_expr_missing_latent_source(state, *elem)),
            HirExpr::Map { entries, .. } => entries
                .iter()
                .any(|entry| self.flow_expr_missing_latent_source(state, entry.value)),
            HirExpr::Record(record) => record.fields.iter().any(|field| match field {
                etas_hir::HirFieldInit::Named { value, .. } => {
                    self.flow_expr_missing_latent_source(state, *value)
                }
                etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                    let ResolveResult::Resolved(symbol) = resolution else {
                        return false;
                    };
                    self.symbol_type_contains_flow(*symbol)
                        && state.local_latent_values.symbol_sources(*symbol).is_none()
                }
            }),
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => args
                .iter()
                .filter_map(arg_expr)
                .any(|arg| self.flow_expr_missing_latent_source(state, arg)),
            _ => false,
        }
    }

    fn flow_expr_missing_latent_source(&self, state: &EffectState, expr: HirExprId) -> bool {
        if !self.expr_type_contains_flow(expr) {
            return false;
        }
        self.latent_sources_for_expr(state, expr).is_none()
    }

    fn expr_type_contains_flow(&self, expr: HirExprId) -> bool {
        self.types
            .facts
            .expr_types
            .get(&expr)
            .is_some_and(|ty| self.type_contains_flow(*ty))
    }

    fn symbol_type_contains_flow(&self, symbol: etas_hir::SymbolId) -> bool {
        self.symbol_value_type(symbol)
            .is_some_and(|ty| self.type_contains_flow(ty))
    }

    fn type_contains_flow(&self, ty: etas_types::TypeId) -> bool {
        match self.types.store.get(ty) {
            Some(Type::Function(_)) => true,
            Some(Type::Array(inner))
            | Some(Type::List(inner))
            | Some(Type::Set(inner))
            | Some(Type::Slice(inner))
            | Some(Type::Option(inner))
            | Some(Type::Schema(inner))
            | Some(Type::Message(inner))
            | Some(Type::MemorySelection(inner))
            | Some(Type::MemoryRegion(inner))
            | Some(Type::Refined { base: inner, .. })
            | Some(Type::Trust { inner, .. }) => self.type_contains_flow(*inner),
            Some(Type::Map { value, .. }) => self.type_contains_flow(*value),
            Some(Type::Result { ok, err }) => {
                self.type_contains_flow(*ok) || self.type_contains_flow(*err)
            }
            Some(Type::Record(record)) => record
                .fields
                .iter()
                .any(|field| self.type_contains_flow(field.ty)),
            Some(Type::Tuple(types)) => types.iter().any(|ty| self.type_contains_flow(*ty)),
            Some(Type::Store { key, value }) => {
                self.type_contains_flow(*key) || self.type_contains_flow(*value)
            }
            Some(Type::Applied { args, .. }) => args.iter().any(|ty| self.type_contains_flow(*ty)),
            Some(
                Type::Primitive(_)
                | Type::IntegerLiteral { .. }
                | Type::Var(_)
                | Type::Range { .. }
                | Type::Enum(_)
                | Type::Handler(_)
                | Type::Named(_)
                | Type::Nominal(_)
                | Type::Prompt
                | Type::PromptPart
                | Type::MemoryPlace(_)
                | Type::ResourceHandle(_),
            )
            | None => false,
        }
    }

    fn is_deferred_first_class_param(&self, expr: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol_data) = self.hir.symbols.get(symbol) else {
            return false;
        };
        if !matches!(symbol_data.kind, etas_hir::SymbolKind::Param)
            && !matches!(symbol_data.def, SymbolDef::Param { .. })
        {
            return false;
        }
        self.flow_type_for_expr(expr)
            .is_some_and(|flow| flow.effects.is_none())
    }

    fn is_projected_flow_value(&self, expr: HirExprId) -> bool {
        matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::Index { .. } | HirExpr::Field { .. } | HirExpr::Call { .. })
        )
    }

    fn apply_deferred_first_class_call_sites(
        &mut self,
        context: UnitContext<EffectUnit>,
        site: CallSite<EffectUnit>,
        callee: EffectUnit,
        mut state: EffectState,
    ) -> EffectState {
        let EffectUnit::Item(item) = callee else {
            return state;
        };
        let deferred = self
            .inputs
            .deferred_first_class_calls
            .get(&callee)
            .cloned()
            .unwrap_or_default();
        let returned_flow = self.returned_anonymous_flow_unit(item);
        let returned_deferred = returned_flow.and_then(|unit| {
            self.inputs
                .deferred_first_class_calls
                .get(&unit)
                .cloned()
                .map(|calls| (unit, calls))
        });
        if let Some(returned_unit) = returned_flow
            && returned_deferred.is_none()
        {
            state
                .local_latent_values
                .record_value(site.call, returned_unit);
        }
        if deferred.is_empty() && returned_deferred.is_none() {
            if returned_flow.is_none()
                && self
                    .flow_type_for_expr(site.call)
                    .is_some_and(|flow| flow.effects.is_none())
            {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    site.span,
                    "returned first-class flow propagation requires a checked latent return source",
                ));
                state.mark_incomplete();
                state.summary.support =
                    InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
                return state;
            }
            return state;
        }
        let call_args =
            match self.hir.exprs.get(site.call) {
                Some(HirExpr::Call { args, .. }) => args.clone(),
                _ => return self.incomplete_at(
                    site.span,
                    "first-class flow call specialization requires a materialized call expression",
                    state,
                ),
            };
        let mut specialized = EffectSummary::local();
        let mut solved_any = false;
        for deferred_call in &deferred {
            match self.specialized_summary_for_deferred_call(
                item,
                *deferred_call,
                &call_args,
                &state,
                site.span,
            ) {
                DeferredSpecialization::Solved(summary) => {
                    let call_unit = EffectUnit::FirstClassFlowCall {
                        owner: item,
                        call: *deferred_call,
                    };
                    self.inputs.unit_effects.insert(call_unit, summary.clone());
                    self.inputs.solved_deferred_units.insert(call_unit);
                    specialized.seq_assign(&summary);
                    solved_any = true;
                }
                DeferredSpecialization::Rejected { message } => {
                    state = self.incomplete_at(site.span, message, state);
                    self.reject_deferred_unit(callee);
                }
            }
        }
        if solved_any {
            state.summary.seq_assign(&specialized);
            self.inputs.solved_deferred_units.insert(callee);
        }

        if let Some((returned_unit, returned_calls)) = returned_deferred {
            let mut returned_summary = EffectSummary::local();
            let mut returned_solved = false;
            for deferred_call in returned_calls {
                match self.specialized_summary_for_deferred_call(
                    item,
                    deferred_call,
                    &call_args,
                    &state,
                    site.span,
                ) {
                    DeferredSpecialization::Solved(summary) => {
                        let deferred_call_unit = EffectUnit::FirstClassFlowCall {
                            owner: item,
                            call: deferred_call,
                        };
                        self.inputs
                            .unit_effects
                            .insert(deferred_call_unit, summary.clone());
                        self.inputs.solved_deferred_units.insert(deferred_call_unit);
                        returned_summary.seq_assign(&summary);
                        returned_solved = true;
                    }
                    DeferredSpecialization::Rejected { .. } => {
                        state = self.incomplete_at(
                            site.span,
                            "returned first-class flow propagation requires a checked latent return source",
                            state,
                        );
                        self.reject_deferred_unit(returned_unit);
                    }
                }
            }
            if returned_solved {
                let owner = context.unit.owner().unwrap_or(item);
                let call_unit = EffectUnit::FirstClassFlowCall {
                    owner,
                    call: site.call,
                };
                self.inputs.unit_effects.insert(call_unit, returned_summary);
                self.inputs.solved_deferred_units.insert(returned_unit);
                state.local_latent_values.record_value(site.call, call_unit);
            }
        }

        state
    }

    fn specialized_summary_for_deferred_call(
        &mut self,
        callee_item: HirItemId,
        deferred_call: HirExprId,
        call_args: &[HirArg],
        state: &EffectState,
        span: Span,
    ) -> DeferredSpecialization {
        let Some(param_symbol) = self.deferred_call_param_symbol(deferred_call) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires a checked parameter target",
            };
        };
        let Some(param_ty) = self.symbol_value_type(param_symbol) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires checked parameter type facts",
            };
        };
        let Some(Type::Function(flow)) = self.types.store.get(param_ty) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires a checked function parameter type",
            };
        };
        if let Some(row) = &flow.effects {
            return DeferredSpecialization::Solved(
                self.summary_from_public_row(self.row_from_type_ref(row), span),
            );
        }
        let Some(index) = self.param_index(callee_item, param_symbol) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires checked parameter type facts",
            };
        };
        let Some(arg) = call_args.get(index).and_then(arg_expr) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires a matching call-site argument",
            };
        };
        if let Some(sources) = self.latent_sources_for_expr(state, arg) {
            let mut summary = EffectSummary::local();
            for source in sources {
                let Some(source_summary) = self.inputs.unit_effects.get(&source).cloned() else {
                    return DeferredSpecialization::Rejected {
                        message: "first-class flow call requires a checked latent effect fact",
                    };
                };
                summary.seq_assign(&source_summary);
                self.record_latent_realization(source, deferred_call);
            }
            return DeferredSpecialization::Solved(summary);
        }
        if let Some(arg_summary) = self.summary_for_flow_expr_type(arg, span) {
            return DeferredSpecialization::Solved(arg_summary);
        }
        DeferredSpecialization::Rejected {
            message: "first-class flow call requires a checked latent effect fact or an explicit function effect row",
        }
    }

    fn deferred_call_param_symbol(&self, call: HirExprId) -> Option<etas_hir::SymbolId> {
        let HirExpr::Call { callee, .. } = self.hir.exprs.get(call)? else {
            return None;
        };
        let HirExpr::Path(path) = self.hir.exprs.get(*callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        Some(symbol)
    }

    pub(crate) fn symbol_value_type(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Option<etas_types::TypeId> {
        match self
            .type_symbols
            .symbol_fact(self.hir, &self.types.facts, symbol)?
        {
            SymbolTypeFact::Param { ty }
            | SymbolTypeFact::Local { ty, .. }
            | SymbolTypeFact::Field { ty }
            | SymbolTypeFact::Value { ty }
            | SymbolTypeFact::TopLevelLet { ty, .. } => Some(*ty),
            _ => None,
        }
    }

    pub(crate) fn pure_type_constructor_call(&self, callee: HirExprId) -> bool {
        let Some(symbol) = self.callee_symbol(callee) else {
            return false;
        };
        if matches!(
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol),
            Some(
                SymbolTypeFact::Type { .. }
                    | SymbolTypeFact::NominalType { .. }
                    | SymbolTypeFact::TypeAlias { .. }
            )
        ) {
            return true;
        }
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return false;
        };
        if let SymbolDef::ImportAlias {
            path,
            origin: etas_hir::ImportAliasOrigin::SourceImport,
        } = &symbol.def
        {
            return self.source_type_item_for_path(path).is_some();
        }
        matches!(
            symbol.kind,
            etas_hir::SymbolKind::Type
                | etas_hir::SymbolKind::TypeAlias
                | etas_hir::SymbolKind::Enum
                | etas_hir::SymbolKind::EnumVariant
        )
    }

    fn param_index(&self, item: HirItemId, symbol: etas_hir::SymbolId) -> Option<usize> {
        match self.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => flow.params.iter().position(|param| *param == symbol),
            etas_hir::HirItem::Agent(agent) => {
                agent.params.iter().position(|param| *param == symbol)
            }
            etas_hir::HirItem::Tool(tool) => tool.params.iter().position(|param| *param == symbol),
            _ => None,
        }
    }

    fn source_type_item_for_path(&self, path: &[String]) -> Option<HirItemId> {
        let (name, module_path) = path.split_last()?;
        for (item, hir_item) in self.hir.items.iter() {
            let symbol = match hir_item {
                etas_hir::HirItem::TypeAlias(item) => item.symbol,
                etas_hir::HirItem::Type(item) => item.symbol,
                etas_hir::HirItem::Enum(item) => item.symbol,
                _ => continue,
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

    fn returned_anonymous_flow_unit(&self, item: HirItemId) -> Option<EffectUnit> {
        let block = self.owner_primary_block(item)?;
        let block = self.hir.blocks.get(block)?;
        for stmt in &block.stmts {
            let Some(HirStmt::Return {
                value: Some(value), ..
            }) = self.hir.stmts.get(*stmt)
            else {
                continue;
            };
            if let Some(unit) = self.anonymous_flow_unit_for_expr(item, *value) {
                return Some(unit);
            }
        }
        block
            .final_expr
            .and_then(|expr| self.anonymous_flow_unit_for_expr(item, expr))
    }

    fn owner_primary_block(&self, item: HirItemId) -> Option<etas_hir::HirBlockId> {
        let body = self.context.item_primary_body(item)?;
        self.context.body_block(body)
    }

    fn reject_deferred_unit(&mut self, unit: EffectUnit) {
        let summary = self.inputs.unit_effects.entry(unit).or_default();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
    }

    fn record_latent_realization(&mut self, source: EffectUnit, call: HirExprId) {
        if let EffectUnit::AnonymousFlow { value, .. } = source {
            let realized_at = self.inputs.latent_realizations.entry(value).or_default();
            if !realized_at.contains(&call) {
                realized_at.push(call);
            }
        }
    }
}

fn external_summary_matches_path(
    summary: &crate::ExternalEffectSummaryMetadata,
    path: &[String],
) -> bool {
    summary.item == path
        && summary
            .import_root
            .as_ref()
            .map(|root| module_path_has_import_root(path, root))
            .unwrap_or(true)
}

fn module_path_has_import_root(path: &[String], root: &str) -> bool {
    let root_segments = root
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    !root_segments.is_empty()
        && path.len() >= root_segments.len()
        && path
            .iter()
            .zip(root_segments)
            .all(|(segment, root_segment)| segment == root_segment)
}

fn limit_kind_from_std(kind: StdLimitKind) -> LimitKind {
    match kind {
        StdLimitKind::Iterations => LimitKind::Iterations,
        StdLimitKind::Tokens => LimitKind::Tokens,
        StdLimitKind::ContextTokens => LimitKind::ContextTokens,
        StdLimitKind::Cost => LimitKind::Cost,
        StdLimitKind::WallTime => LimitKind::WallTime,
        StdLimitKind::Attempts => LimitKind::Attempts,
    }
}

fn annotation_name(annotation: &etas_hir::HirAnnotation) -> String {
    annotation
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::{EffectSemantics, external_summary_matches_path};
    use etas_core::SourceId;
    use etas_hir::{HirExpr, HirLiteral, lower_program};
    use etas_hir_analysis::HirAnalysisContext;
    use etas_types::{EffectArgRef, TypeOutput};

    #[test]
    fn external_summary_import_root_matches_multi_segment_prefix() {
        let summary = crate::ExternalEffectSummaryMetadata {
            package: Some("company-agents@1.0.0#company.agents".to_owned()),
            import_root: Some("company.agents".to_owned()),
            item: vec![
                "company".to_owned(),
                "agents".to_owned(),
                "writer".to_owned(),
                "run".to_owned(),
            ],
            param_names: Vec::new(),
            public_effects: Default::default(),
            requested_actions: Default::default(),
            handled_requested_actions: Default::default(),
            latent_flows: Vec::new(),
        };

        assert!(external_summary_matches_path(
            &summary,
            &["company", "agents", "writer", "run"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        ));
        assert!(!external_summary_matches_path(
            &summary,
            &["company", "other", "writer", "run"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        ));
    }

    #[test]
    fn external_summary_url_host_projection_refines_from_literal_url_parameter() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            r#"
flow get(url: string) -> unit {
  return;
}

flow main() -> unit {
  get("https://example.com/status");
  return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let url = hir
            .exprs
            .iter()
            .find_map(|(expr, data)| match data {
                HirExpr::Literal(HirLiteral::String { value, .. })
                    if value == "https://example.com/status" =>
                {
                    Some(expr)
                }
                _ => None,
            })
            .expect("literal URL expression should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            crate::EffectRegistry::with_standard_effects(),
            &[],
            &[],
        );

        let refined = semantics.specialize_effect_arg(
            EffectArgRef::Path(vec!["req".into(), "url".into(), "host".into()]),
            &[("url".to_owned(), url)],
            &[],
        );

        assert_eq!(refined, EffectArgRef::String("example.com".to_owned()));
    }

    #[test]
    fn url_literal_projection_refines_parse_result_url_host() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            r#"
flow parse_url(input: string) -> i32 {
  return 0;
}

flow main() -> unit {
  parse_url("https://example.com/status");
  return;
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let call = hir
            .exprs
            .iter()
            .find_map(|(_, data)| match data {
                HirExpr::Call { callee, args, .. } => Some((*callee, args.clone())),
                _ => None,
            })
            .expect("parse_url call should lower");
        let types = TypeOutput::default();
        let context = HirAnalysisContext::new(&hir);
        let semantics = EffectSemantics::with_context(
            &hir,
            context,
            &types,
            crate::EffectRegistry::with_standard_effects(),
            &[],
            &[],
        );

        let host = semantics.string_arg_from_call_path(
            call.0,
            &call.1,
            &["url".to_owned(), "host".to_owned()],
            &[],
            super::EFFECT_ARG_EVAL_DEPTH_LIMIT,
        );

        assert_eq!(host, Some("example.com".to_owned()));
    }
}

enum DeferredSpecialization {
    Solved(EffectSummary),
    Rejected { message: &'static str },
}

fn arg_expr(arg: &HirArg) -> Option<HirExprId> {
    match arg {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => Some(*expr),
    }
}

fn insert_type_binding(bindings: &mut Vec<(String, TypeId)>, name: String, ty: TypeId) -> bool {
    if bindings.iter().any(|(existing, _)| existing == &name) {
        return false;
    }
    bindings.push((name, ty));
    true
}

fn named_type_name(store: &etas_types::TypeStore, ty: TypeId) -> Option<String> {
    match store.get(ty)? {
        Type::Named(name) => Some(name.name.clone()),
        _ => None,
    }
}

fn collect_type_bindings_from_type_pattern(
    store: &etas_types::TypeStore,
    pattern: TypeId,
    actual: TypeId,
    allowed_names: &[String],
    bindings: &mut Vec<(String, TypeId)>,
) -> bool {
    match store.get(pattern) {
        Some(Type::Named(name)) if allowed_names.iter().any(|allowed| allowed == &name.name) => {
            insert_type_binding(bindings, name.name.clone(), actual)
        }
        Some(Type::Array(pattern)) => match store.get(actual) {
            Some(Type::Array(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::List(pattern)) => match store.get(actual) {
            Some(Type::List(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Set(pattern)) => match store.get(actual) {
            Some(Type::Set(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Slice(pattern)) => match store.get(actual) {
            Some(Type::Slice(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Option(pattern)) => match store.get(actual) {
            Some(Type::Option(actual)) => collect_type_bindings_from_type_pattern(
                store,
                *pattern,
                *actual,
                allowed_names,
                bindings,
            ),
            _ => false,
        },
        Some(Type::Result {
            ok: pattern_ok,
            err: pattern_err,
        }) => match store.get(actual) {
            Some(Type::Result {
                ok: actual_ok,
                err: actual_err,
            }) => {
                let ok_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_ok,
                    *actual_ok,
                    allowed_names,
                    bindings,
                );
                let err_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_err,
                    *actual_err,
                    allowed_names,
                    bindings,
                );
                ok_changed || err_changed
            }
            _ => false,
        },
        Some(Type::Map {
            key: pattern_key,
            value: pattern_value,
        })
        | Some(Type::Store {
            key: pattern_key,
            value: pattern_value,
        }) => match store.get(actual) {
            Some(Type::Map {
                key: actual_key,
                value: actual_value,
            })
            | Some(Type::Store {
                key: actual_key,
                value: actual_value,
            }) => {
                let key_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_key,
                    *actual_key,
                    allowed_names,
                    bindings,
                );
                let value_changed = collect_type_bindings_from_type_pattern(
                    store,
                    *pattern_value,
                    *actual_value,
                    allowed_names,
                    bindings,
                );
                key_changed || value_changed
            }
            _ => false,
        },
        Some(Type::Tuple(pattern_elements)) => match store.get(actual) {
            Some(Type::Tuple(actual_elements))
                if pattern_elements.len() == actual_elements.len() =>
            {
                pattern_elements
                    .iter()
                    .copied()
                    .zip(actual_elements.iter().copied())
                    .fold(false, |changed, (pattern, actual)| {
                        collect_type_bindings_from_type_pattern(
                            store,
                            pattern,
                            actual,
                            allowed_names,
                            bindings,
                        ) || changed
                    })
            }
            _ => false,
        },
        Some(Type::Applied {
            constructor: pattern_constructor,
            args: pattern_args,
        }) => match store.get(actual) {
            Some(Type::Applied {
                constructor: actual_constructor,
                args: actual_args,
            }) if pattern_constructor == actual_constructor
                && pattern_args.len() == actual_args.len() =>
            {
                pattern_args
                    .iter()
                    .copied()
                    .zip(actual_args.iter().copied())
                    .fold(false, |changed, (pattern, actual)| {
                        collect_type_bindings_from_type_pattern(
                            store,
                            pattern,
                            actual,
                            allowed_names,
                            bindings,
                        ) || changed
                    })
            }
            _ => false,
        },
        _ => false,
    }
}

fn host_from_absolute_url(value: &str) -> Option<String> {
    let (_, rest) = value.split_once("://")?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host_port)| host_port)
        .unwrap_or(authority);
    if let Some(stripped) = host_port.strip_prefix('[') {
        let (host, _) = stripped.split_once(']')?;
        return (!host.is_empty()).then(|| host.to_owned());
    }
    let host = host_port
        .split_once(':')
        .map(|(host, _)| host)
        .unwrap_or(host_port);
    (!host.is_empty()).then(|| host.to_owned())
}
