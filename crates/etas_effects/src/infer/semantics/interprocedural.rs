use super::engine::EffectSemantics;
use super::shared::*;

impl InterproceduralSemantics for EffectSemantics<'_> {
    type Unit = EffectUnit;
    type Domain = EffectState;
    type Summary = EffectSummary;

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody {
        unit.analysis_body_with_context(&self.context)
    }

    fn begin_unit(&mut self, context: UnitContext<Self::Unit>) -> Self::Domain {
        let mut state = EffectState::for_unit(context.unit);
        if let Some(captures) = self.captured_values.get(&context.unit) {
            state.local_latent_values.join_assign(captures);
        }
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
        let exits = self.capture_exits(context.unit, &exit);
        let state = exit.into_joined_domain();
        if let EffectUnit::Item(item) = context.unit
            && let Some(etas_hir::HirItem::TopLevelLet(binding)) = self.hir.items.get(item)
            && let Some(sources) = self.latent_sources_for_expr(&state, binding.value)
        {
            self.discovered_global_values
                .record_symbol_sources(binding.symbol, sources);
        }
        let mut summary = state.summary;
        summary.exits = Some(exits);
        if let EffectUnit::Item(item) = context.unit
            && let Some(method) = self
                .types
                .facts
                .agent_methods
                .values()
                .find(|method| method.body == item)
        {
            self.apply_agent_annotation_requirements(method.agent, &mut summary);
        }
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
        self.callable_summaries.get_mut().invalidate();
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

    fn method_call_target(
        &mut self,
        _context: UnitContext<Self::Unit>,
        call: HirExprId,
        _state: &Self::Domain,
    ) -> Option<Self::Unit> {
        let symbol = self.types.facts.agent_method_calls.get(&call)?;
        let method = self.types.facts.agent_methods.get(symbol)?;
        let target = EffectUnit::Item(method.body);
        self.inputs
            .native_call_targets
            .entry(call)
            .or_default()
            .insert(target);
        Some(target)
    }

    fn spec_method_call_target(
        &mut self,
        _context: UnitContext<Self::Unit>,
        call: HirExprId,
        _state: &Self::Domain,
    ) -> Option<Self::Unit> {
        let target = self.checked_spec_method_target(call)?;
        self.inputs
            .native_call_targets
            .entry(call)
            .or_default()
            .insert(target);
        Some(target)
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

    fn expression_dependencies(
        &mut self,
        context: UnitContext<Self::Unit>,
        expr: HirExprId,
    ) -> Vec<Self::Unit> {
        if let Some(HirExpr::SpecMethodCall {
            spec_path, method, ..
        }) = self.hir.exprs.get(expr)
        {
            if let ResolveResult::Resolved(spec) = spec_path.resolution
                && let Some(spec) = self.type_symbols.canonical_symbol(self.hir, spec)
            {
                // Discover candidate bodies so later evidence specialization can
                // choose one solved summary. This does not merge their behavior.
                let mut dependencies = std::collections::BTreeSet::new();
                for implementation in &self.types.facts.spec_impls {
                    if implementation.spec_symbol != spec {
                        continue;
                    }
                    for candidate in implementation
                        .methods
                        .iter()
                        .filter(|candidate| candidate.name == *method)
                    {
                        if let Some(SymbolDef::Item { item }) = self
                            .hir
                            .symbols
                            .get(candidate.symbol)
                            .map(|symbol| &symbol.def)
                        {
                            dependencies.insert(EffectUnit::Item(*item));
                        }
                    }
                }
                return dependencies.into_iter().collect();
            }
        }
        if matches!(self.hir.exprs.get(expr), Some(HirExpr::StageCompose { .. }))
            && let Some(owner) = context.unit.owner()
            && let Some(unit) = self.anonymous_flow_unit_for_expr(owner, expr)
            && unit != context.unit
        {
            return vec![unit];
        }
        if let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) {
            let symbol = match &path.resolution {
                ResolveResult::Resolved(symbol) => Some(*symbol),
                ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix,
                _ => None,
            };
            if let Some(symbol) =
                symbol.and_then(|symbol| self.type_symbols.canonical_symbol(self.hir, symbol))
            {
                match self.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
                    Some(SymbolDef::TopLevelLet {
                        item,
                        classification,
                        ..
                    }) if !matches!(
                        classification,
                        etas_hir::TopLevelLetClassification::ResourceHandle(_)
                    ) =>
                    {
                        return vec![EffectUnit::Item(*item)];
                    }
                    Some(SymbolDef::Item { item }) => {
                        // References depend on the callable's solved behavior,
                        // but do not invoke it or add its actions to this body.
                        let target = match self.hir.items.get(*item) {
                            Some(etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_)) => {
                                Some(EffectUnit::Item(*item))
                            }
                            Some(etas_hir::HirItem::Agent(_)) => {
                                match self.agent_default_call_target(*item) {
                                    CallTarget::Direct(unit) => Some(unit),
                                    _ => None,
                                }
                            }
                            _ => None,
                        };
                        if let Some(unit) = target
                            && !matches!(unit, EffectUnit::Item(item) if self.frozen_calls.get(&item).is_some_and(crate::FrozenCallableSummary::uses_snapshot))
                        {
                            return vec![unit];
                        }
                    }
                    Some(SymbolDef::ImportAlias {
                        path,
                        origin: etas_hir::ImportAliasOrigin::SourceImport,
                    }) => {
                        return self.external_method_dependencies(path);
                    }
                    _ => {}
                }
            }
        }
        let captured_invocation = matches!(context.unit, EffectUnit::AnonymousFlow { value, .. } if value == expr)
            || matches!(self.hir.exprs.get(expr), Some(HirExpr::Pipeline { .. }));
        if captured_invocation && let Some(captures) = self.captured_values.get(&context.unit) {
            let mut dependencies = std::collections::BTreeSet::new();
            let sources = match self.hir.exprs.get(expr) {
                Some(HirExpr::Pipeline { stages, .. }) => stages
                    .iter()
                    .filter_map(|stage| captures.value_sources(stage.expr))
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>(),
                _ => captures.values.values().flatten().copied().collect(),
            };
            for source in sources {
                if let LatentSource::Unit(unit) | LatentSource::Reference { unit, .. } = source
                    && !matches!(unit, EffectUnit::Item(item) if self.frozen_calls.get(&item).is_some_and(crate::FrozenCallableSummary::uses_snapshot))
                {
                    dependencies.insert(unit);
                }
            }
            return dependencies.into_iter().collect();
        }
        let Some(fact) = self.types.facts.agent_inferences.get(&expr) else {
            return Vec::new();
        };
        match self.agent_model_tools(fact.agent) {
            Some(tools) => tools
                .into_iter()
                .filter_map(|tool| match tool {
                    super::inference::ModelToolSource::Item(item) => Some(EffectUnit::Item(item)),
                    super::inference::ModelToolSource::External(_) => None,
                })
                .collect(),
            // Discovery cannot supply a missing tool fact. The inference
            // transfer rejects it when applying the checked summaries.
            None => Vec::new(),
        }
    }

    fn direct_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        callee: Self::Unit,
        summary: &Self::Summary,
        mut state: Self::Domain,
    ) -> Self::Domain {
        let frozen = if let EffectUnit::Item(item) = callee {
            self.frozen_calls.get(&item).cloned()
        } else {
            None
        };
        let summary = match &frozen {
            Some(crate::FrozenCallableSummary::Solved(summary)) => summary.as_ref(),
            Some(crate::FrozenCallableSummary::AnalyzePinnedSource) => summary,
            Some(crate::FrozenCallableSummary::Unavailable) => {
                return self.incomplete_at(
                    site.span,
                    "frozen callable has no solved effect summary",
                    state,
                );
            }
            None => summary,
        };
        state = self.observe_inferred_callback_arguments(callee, &site, state);
        if let Err(error) =
            self.record_returned_callable_sources(callee, &site, summary, &mut state, None)
        {
            return self.incomplete_at(site.span, error.to_string(), state);
        }
        let summary = match self.specialize_summary_for_call(callee, &site, &state, summary, None) {
            Ok(summary) => summary,
            Err(error) => {
                return self.incomplete_at(
                    site.span,
                    format!("call effect selector specialization failed: {error}"),
                    state,
                );
            }
        };
        self.note_call_outcomes(site.call, callee, &summary, &mut state);
        state.summary.seq_assign(&summary);
        state = self.validate_returned_flow_source(&site, state);
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
            let mut realized: Option<EffectSummary> = None;
            let mut parameter_calls = std::collections::BTreeSet::new();
            let mut other_unproved_control = false;
            let returns_flow = self.expr_type_contains_flow(site.call);
            for source in sources {
                let mut summary = match self.latent_source_summary(source, site.span) {
                    Ok(summary) => summary,
                    Err(error) => return self.incomplete_at(site.span, error.to_string(), state),
                };
                if let LatentSource::Metadata(source) = source {
                    summary = match self.metadata_value_call(source, &site, &mut state) {
                        Ok(summary) => summary,
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                    };
                } else if let LatentSource::StdReference { value, .. } = source {
                    match self.record_std_reference_result(value, &site, &mut state) {
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                        Ok(false) if returns_flow => {
                            state.local_latent_values.record_value_sources(
                                site.call,
                                [LatentSource::Unavailable { expr: site.call }],
                            );
                        }
                        _ => {}
                    }
                } else if let LatentSource::Unit(unit)
                | LatentSource::Reference { unit, .. }
                | LatentSource::Captured { unit, .. } = source
                {
                    let environment = match source {
                        LatentSource::Captured { environment, .. } => Some(environment),
                        LatentSource::Reference { .. } => {
                            match self.reference_invocation_environment(source) {
                                Ok(environment) => environment,
                                Err(error) => {
                                    return self.incomplete_at(site.span, error.to_string(), state);
                                }
                            }
                        }
                        _ => None,
                    };
                    match self.record_returned_callable_sources(
                        unit,
                        &site,
                        &summary,
                        &mut state,
                        environment,
                    ) {
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                        Ok(false) if returns_flow => {
                            state.local_latent_values.record_value_sources(
                                site.call,
                                [LatentSource::Unavailable { expr: site.call }],
                            );
                        }
                        _ => {}
                    }
                    summary = match self.specialize_summary_for_call(
                        unit,
                        &site,
                        &state,
                        &summary,
                        environment,
                    ) {
                        Ok(summary) => summary,
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                    };
                    state = self.validate_returned_flow_source(&site, state);
                } else if returns_flow {
                    state.local_latent_values.record_value_sources(
                        site.call,
                        [LatentSource::Unavailable { expr: site.call }],
                    );
                }
                if !summary
                    .exits
                    .as_ref()
                    .is_some_and(crate::CallableExitSummary::returns_only)
                {
                    if let LatentSource::Parameter { symbol, .. } = source {
                        let Some(output) = self.types.facts.expr_types.get(&site.call).copied()
                        else {
                            return self.incomplete_at(
                                site.span,
                                "callback completion requires the checked call output type",
                                state,
                            );
                        };
                        parameter_calls.insert(crate::OutcomeAnalysisGap::ParameterCall {
                            target: crate::ParameterCallTarget::Native(symbol),
                            output,
                            source: site.span.into(),
                        });
                    } else {
                        other_unproved_control = true;
                    }
                }
                if let Some(existing) = &mut realized {
                    existing.join_branch(&summary);
                } else {
                    realized = Some(summary);
                }
                if let LatentSource::Unit(source) = source {
                    self.record_latent_realization(source, site.call);
                }
            }
            if let Some(summary) = realized {
                if !summary
                    .exits
                    .as_ref()
                    .is_some_and(crate::CallableExitSummary::returns_only)
                {
                    if !other_unproved_control && !parameter_calls.is_empty() {
                        state.exits.gaps.extend(parameter_calls);
                    } else {
                        state
                            .exits
                            .gaps
                            .insert(crate::OutcomeAnalysisGap::DynamicCall(site.call));
                    }
                }
                state.summary.seq_assign(&summary);
                return state;
            }
        }
        state
            .exits
            .gaps
            .insert(crate::OutcomeAnalysisGap::DynamicCall(site.call));
        if self.is_deferred_first_class_param(site.callee_expr) {
            self.inputs
                .deferred_first_class_calls
                .entry(context.unit)
                .or_default()
                .push(site.call);
            state.mark_incomplete();
            return state;
        }
        if let Some(flow) = self.flow_type_for_expr(site.callee_expr) {
            if flow.effects.is_none() && self.is_projected_flow_value(site.callee_expr) {
                return self.reject_effect_state(
                    site.span,
                    "latent flow realization requires checked latent source facts",
                    state,
                );
            }
            return self.incomplete_at(
                site.span,
                "first-class flow call requires a checked latent effect fact; an escaping row does not describe requested actions",
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
        context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        mut state: Self::Domain,
    ) -> Self::Domain {
        if let Some(item) = self.frozen_call_targets.get(&site.call).copied() {
            match self.frozen_calls.get(&item).cloned() {
                Some(crate::FrozenCallableSummary::Solved(summary)) => {
                    return InterproceduralSemantics::direct_call(
                        self,
                        context,
                        site,
                        EffectUnit::Item(item),
                        &summary,
                        state,
                    );
                }
                _ => {
                    return self.incomplete_at(
                        site.span,
                        "frozen callable has no solved effect summary",
                        state,
                    );
                }
            }
        }
        if !self.is_nominal_constructor_callee(site.callee_expr)
            && !self.pure_type_constructor_call(site.callee_expr)
        {
            self.note_external_outcomes(site.call, &mut state);
        }
        self.apply_external_call(site, state)
    }

    fn incomplete_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        mut state: Self::Domain,
    ) -> Self::Domain {
        if self.is_nominal_constructor_callee(site.callee_expr)
            || self.pure_type_constructor_call(site.callee_expr)
        {
            return state;
        }
        self.note_external_outcomes(site.call, &mut state);
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
