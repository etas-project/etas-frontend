use super::engine::EffectSemantics;
use super::shared::*;

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
        let summary = match self.specialize_summary_for_call(callee, &site, &state, summary) {
            Ok(summary) => summary,
            Err(error) => {
                return self.incomplete_at(
                    site.span,
                    format!("call effect selector specialization failed: {error}"),
                    state,
                );
            }
        };
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
