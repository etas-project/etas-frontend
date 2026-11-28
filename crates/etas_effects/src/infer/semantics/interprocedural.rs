use super::engine::EffectSemantics;
use super::shared::*;

impl InterproceduralSemantics for EffectSemantics<'_> {
    type Unit = EffectUnit;
    type Domain = EffectState;
    type Summary = EffectSummary;

    fn select_unit(&mut self, unit: EffectUnit) {
        self.select_reader(unit.owner());
    }

    fn begin_component(&mut self, units: &[EffectUnit], cyclic: bool) {
        self.trace_equations.begin(units, cyclic);
    }

    fn end_component(
        &mut self,
        units: &[EffectUnit],
        summaries: &mut etas_hir_analysis::interprocedural::SummaryStore<EffectUnit, EffectSummary>,
    ) {
        for unit in units {
            if let Some(summary) = summaries.get_mut(*unit) {
                self.trace_equations.close_summary(summary);
                // Publish each solved component as immutable subgraphs. Direct
                // calls must share them just as first-class callable queries do.
                summary.action_trace = std::mem::take(&mut summary.action_trace).shared();
                if let Some(exits) = &mut summary.exits {
                    for exit in &mut exits.exits {
                        exit.trace = std::mem::take(&mut exit.trace).shared();
                    }
                    for exit in &mut exits.parameter_exits {
                        exit.prefix = std::mem::take(&mut exit.prefix).shared();
                    }
                }
                self.inputs.unit_effects.insert(*unit, summary.clone());
            }
        }
        if !self.trace_equations.has_active_component() {
            return;
        }
        self.trace_equations.close_facts(&mut self.inputs);
        for fact in self.inputs.latent_effects.values_mut() {
            self.trace_equations.close_summary(&mut fact.summary);
        }
        for fact in self.inputs.std_callable_values.values_mut() {
            self.trace_equations.close_summary(&mut fact.summary);
        }
        self.callable_summaries.get_mut().invalidate();
    }

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody {
        unit.analysis_body_with_context(&self.context)
    }

    fn unit_dependencies(&self, unit: Self::Unit) -> Vec<Self::Unit> {
        unit.owner()
            .and_then(|owner| self.spec_unit_targets.get(&owner))
            .into_iter()
            .flatten()
            .copied()
            .collect()
    }

    fn begin_unit(&mut self, context: UnitContext<Self::Unit>) -> Self::Domain {
        // Retain all control-flow alternatives of the current analysis, rather
        // than provisional reads from an earlier interprocedural iteration.
        if let Some(previous) = self.checked_read_expressions.remove(&context.unit) {
            let reads = self.discovered_call_results.get_mut();
            for expr in previous {
                reads.remove_value(expr);
            }
        }
        let mut state = EffectState::for_unit(context.unit);
        if let EffectUnit::Item(item) = context.unit
            && self
                .type_queries
                .active_agent_methods
                .get_or_init(|| {
                    self.types
                        .facts
                        .type_impl_methods
                        .values()
                        .filter(|method| {
                            self.types
                                .facts
                                .is_active_agent(&self.types.store, method.self_type)
                        })
                        .map(|method| method.body)
                        .collect()
                })
                .contains(&item)
        {
            state
                .summary
                .require_runtime(crate::RuntimeRequirementReason::TaskScheduler);
            state.summary.determinism = crate::Determinism::NonDeterministic;
            state.summary.independent_determinism = Some(crate::Determinism::NonDeterministic);
        }

        if let EffectUnit::Item(owner) = context.unit
            && let Err(error) = self.record_agent_receiver_projections(owner, &mut state)
        {
            state = self.incomplete_at(self.hir.items[owner].span(), error.to_string(), state);
        }
        if let Some(captures) = self.captured_values.get(&context.unit) {
            state.local_latent_values.join_assign(captures);
        }
        if let EffectUnit::HandlerArm { arm, .. } = context.unit {
            state.handler_depth = 1;
            self.bind_inference_decoder(arm, &mut state);
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
            && let Some(initializer) = binding.value
            && let Some(sources) = self.latent_sources_for_expr(&state, initializer)
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
        self.callable_summaries.get_mut().invalidate();
        summary
    }

    fn stabilize_summary(
        &mut self,
        context: UnitContext<Self::Unit>,
        mut summary: Self::Summary,
        current: Option<&Self::Summary>,
    ) -> Self::Summary {
        self.trace_equations.summarize(context.unit, &mut summary);
        self.inputs
            .unit_effects
            .insert(context.unit, summary.clone());
        summary.stabilize_for_fixpoint(current);
        summary
    }

    fn external_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        match context.unit {
            EffectUnit::Item(item) => {
                let span = self.hir.items[item].span();
                if let Some(etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
                    body: etas_hir::HirFlowBody::Runtime { role, .. },
                    ..
                })) = self.hir.items.get(item)
                {
                    let mut summary = EffectSummary::local();
                    let outcome = match role {
                        etas_std::StdExecutionRole::TaskNextCompletion
                        | etas_std::StdExecutionRole::TaskCancel => {
                            summary.require_runtime(crate::RuntimeRequirementReason::TaskScheduler);
                            summary.determinism = crate::Determinism::NonDeterministic;
                            summary.independent_determinism =
                                Some(crate::Determinism::NonDeterministic);
                            let etas_types::ItemSignature::Flow(signature) =
                                &self.types.facts.item_signatures[&item]
                            else {
                                unreachable!()
                            };
                            crate::CallableOutcome::Returned {
                                ty: signature.output,
                                observation: crate::ReturnObservation::Any,
                            }
                        }
                        etas_std::StdExecutionRole::TaskStart
                        | etas_std::StdExecutionRole::ScopeEnter
                        | etas_std::StdExecutionRole::ResourceScope => {
                            let etas_hir::HirItem::Flow(flow) = &self.hir.items[item] else {
                                unreachable!()
                            };
                            summary = self
                                .summary_for_item_signature(item, span)
                                .expect("task callback signature");
                            summary.require_runtime(crate::RuntimeRequirementReason::TaskScheduler);
                            summary.independent_determinism =
                                Some(crate::Determinism::Deterministic);
                            let body = if *role == etas_std::StdExecutionRole::ScopeEnter {
                                flow.params[1]
                            } else {
                                flow.params[0]
                            };
                            let target = crate::ParameterCallTarget::Native(body);
                            summary.handler_requests =
                                crate::RequestInstantiationSummary::parameter(
                                    target,
                                    "body".into(),
                                    span,
                                );
                            summary.action_trace = crate::ActionTraceDomain::ParameterCall {
                                target,
                                parameter: "body".into(),
                                span,
                            };
                            if *role == etas_std::StdExecutionRole::ResourceScope
                                && let Some(&release) = flow.params.get(1)
                            {
                                // A scope invokes this callback after its body only when
                                // acquisition or transfer left a confirmed live generation.
                                let target = crate::ParameterCallTarget::Native(release);
                                let parameter = self
                                    .hir
                                    .symbols
                                    .get(release)
                                    .expect("scope parameter")
                                    .name
                                    .clone();
                                summary.handler_requests.union_assign(
                                    &crate::RequestInstantiationSummary::parameter(
                                        target,
                                        parameter.clone(),
                                        span,
                                    ),
                                );
                                let mut cleanup = crate::ActionTraceDomain::Empty;
                                cleanup.choice_assign(crate::ActionTraceDomain::ParameterCall {
                                    target,
                                    parameter,
                                    span,
                                });
                                summary.action_trace.seq_assign(cleanup);
                            }
                            let etas_types::ItemSignature::Flow(signature) =
                                &self.types.facts.item_signatures[&item]
                            else {
                                unreachable!()
                            };
                            crate::CallableOutcome::Returned {
                                ty: signature.output,
                                observation: crate::ReturnObservation::Any,
                            }
                        }
                        etas_std::StdExecutionRole::Abort => crate::CallableOutcome::NonReturn,
                        etas_std::StdExecutionRole::Requeue | etas_std::StdExecutionRole::Park => {
                            summary.require_runtime(crate::RuntimeRequirementReason::TaskScheduler);
                            crate::CallableOutcome::NonReturn
                        }
                        etas_std::StdExecutionRole::StorageContext
                        | etas_std::StdExecutionRole::StorageNonce
                        | etas_std::StdExecutionRole::CachedSchemaFingerprint
                        | etas_std::StdExecutionRole::RememberSchemaFingerprint
                        | etas_std::StdExecutionRole::StreamClock
                        | etas_std::StdExecutionRole::ReadScopeEnter
                        | etas_std::StdExecutionRole::CurrentSession
                        | etas_std::StdExecutionRole::MessageContextCapture
                        | etas_std::StdExecutionRole::MessageContextRecord => {
                            // Local execution observations need no external host adapter,
                            // but must never qualify as deterministic CPU-worker work.
                            summary.determinism = crate::Determinism::NonDeterministic;
                            summary.independent_determinism =
                                Some(crate::Determinism::NonDeterministic);
                            let etas_types::ItemSignature::Flow(signature) =
                                &self.types.facts.item_signatures[&item]
                            else {
                                unreachable!()
                            };
                            crate::CallableOutcome::Returned {
                                ty: signature.output,
                                observation: crate::ReturnObservation::Any,
                            }
                        }
                        etas_std::StdExecutionRole::CaptureData
                        | etas_std::StdExecutionRole::CaptureStorage
                        | etas_std::StdExecutionRole::RestoreStorage
                        | etas_std::StdExecutionRole::StorageSchema
                        | etas_std::StdExecutionRole::PayloadConvert
                        | etas_std::StdExecutionRole::ParseBody
                        | etas_std::StdExecutionRole::AgentWithBody
                        | etas_std::StdExecutionRole::CallableWithBody
                        | etas_std::StdExecutionRole::ToolReference
                        | etas_std::StdExecutionRole::ComparatorCapture
                        | etas_std::StdExecutionRole::ArrayTruncate
                        | etas_std::StdExecutionRole::CachedMapIndex
                        | etas_std::StdExecutionRole::RememberMapIndex
                        | etas_std::StdExecutionRole::ComparatorSame => {
                            let etas_types::ItemSignature::Flow(signature) =
                                &self.types.facts.item_signatures[&item]
                            else {
                                unreachable!()
                            };
                            crate::CallableOutcome::Returned {
                                ty: signature.output,
                                observation: crate::ReturnObservation::Any,
                            }
                        }
                        etas_std::StdExecutionRole::Checkpoint => {
                            summary.require_runtime(crate::RuntimeRequirementReason::Checkpoint);
                            let etas_types::ItemSignature::Flow(signature) =
                                &self.types.facts.item_signatures[&item]
                            else {
                                unreachable!()
                            };
                            crate::CallableOutcome::Returned {
                                ty: signature.output,
                                observation: crate::ReturnObservation::Unit,
                            }
                        }
                    };
                    summary.exits = Some(crate::CallableExitSummary {
                        exits: vec![crate::ExitTraceFact {
                            outcome,
                            trace: summary.action_trace.clone(),
                        }],
                        ..Default::default()
                    });
                    if *role == etas_std::StdExecutionRole::ResourceScope {
                        // The VM returns the callback's actual value inside
                        // the source-owned report. Keep its checked result
                        // relation instead of replacing generic R by Any.
                        let etas_hir::HirItem::Flow(flow) = &self.hir.items[item] else {
                            unreachable!()
                        };
                        let projection = match self.intern_parameter_projection(
                            item,
                            flow.params[0],
                            vec![etas_code::descriptor::ValueProjectionStep::CallResult],
                        ) {
                            Ok(projection) => projection,
                            Err(error) => return self.incomplete_summary(span, error.to_string()),
                        };
                        summary.exits.as_mut().unwrap().value_projections.insert(
                            vec![etas_code::descriptor::ValueProjectionStep::Field(
                                "value".into(),
                            )],
                            crate::ProjectedReturnSources::Known(
                                [crate::CallableReturnSource::Projection(projection)].into(),
                            ),
                        );
                    }
                    self.inputs
                        .unit_effects
                        .insert(context.unit, summary.clone());
                    self.callable_summaries.get_mut().invalidate();
                    return summary;
                }
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
        let target = self.checked_method_target(call)?;
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
        let target = self.checked_method_target(call)?;
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
        let mut dependencies = (|| {
            if let Ok(Some(main)) = self.autonomous_entry(expr) {
                match main.declaration.target {
                    etas_types::method::MethodTarget::Source { callable, .. }
                    | etas_types::method::MethodTarget::Agent { callable, .. } => {
                        return vec![EffectUnit::Item(callable)];
                    }
                    _ => {}
                }
            }
            if let Some(targets) = self.spec_method_targets.get(&expr) {
                return targets.iter().copied().collect();
            }
            if let Some(pack) = self.types.facts.existential_packs.get(&expr) {
                return pack
                    .witnesses
                    .iter()
                    .filter_map(|witness| match witness.target {
                        etas_types::method::MethodTarget::Source { callable, .. }
                        | etas_types::method::MethodTarget::Agent { callable, .. } => {
                            Some(EffectUnit::Item(callable))
                        }
                        _ => None,
                    })
                    .collect();
            }
            if let Some(target) = self.checked_method_target(expr) {
                return vec![target];
            }
            if let Some(fact) = self.types.facts.reflections.get(&expr) {
                use etas_types::facts::reflection::CheckedReflectionTarget;
                let declaration = match fact.target {
                    CheckedReflectionTarget::Callable { declaration }
                    | CheckedReflectionTarget::Agent { declaration, .. }
                    | CheckedReflectionTarget::AgentDefaultEntry { declaration, .. } => {
                        Some(declaration)
                    }
                    _ => None,
                };
                if let Some(symbol) = declaration
                    .and_then(|symbol| self.type_symbols.canonical_symbol(self.hir, symbol))
                    && let Some(SymbolDef::Item { item }) =
                        self.hir.symbols.get(symbol).map(|symbol| &symbol.def)
                {
                    let unit =
                        if matches!(self.hir.items.get(*item), Some(etas_hir::HirItem::Agent(_))) {
                            match self.agent_default_call_target(*item) {
                                CallTarget::Direct(unit) => Some(unit),
                                _ => None,
                            }
                        } else {
                            Some(EffectUnit::Item(*item))
                        };
                    if let Some(unit) = unit
                        && self.analysis_units.contains(&unit)
                        && !matches!(unit, EffectUnit::Item(item) if self.frozen_calls.get(&item).is_some_and(crate::FrozenCallableSummary::uses_snapshot))
                    {
                        return vec![unit];
                    }
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
                    if let Some(unit) = source.captured_call_target()
                        && !matches!(unit, EffectUnit::Item(item) if self.frozen_calls.get(&item).is_some_and(crate::FrozenCallableSummary::uses_snapshot))
                    {
                        dependencies.insert(unit);
                    }
                }
                return dependencies.into_iter().collect();
            }
            let mut agents = std::collections::BTreeSet::new();
            if let Some(agent) = self
                .types
                .facts
                .agent_inferences
                .get(&expr)
                .map(|fact| (fact.agent, fact.owner))
                .or_else(|| self.tool_invocation_agent(expr).ok().flatten())
            {
                agents.insert(agent);
            }
            for (_, context) in self.call_generic_bindings(expr).types {
                if let Some(etas_types::Type::AgentContext(etas_types::AgentContextRef::Source(
                    method,
                ))) = self.types.store.get(context)
                    && let Some(method) = self.types.facts.agent_methods.get(method)
                {
                    agents.insert((method.agent, method.body));
                }
            }
            let mut dependencies = std::collections::BTreeSet::new();
            for (agent, method) in agents {
                if let Some(tools) = self.agent_model_tools(agent, method) {
                    for tool in tools {
                        if let super::inference::ModelToolSource::Item(item) = tool {
                            dependencies.insert(EffectUnit::Item(item));
                            if let Some(etas_hir::HirItem::Tool(tool)) = self.hir.items.get(item) {
                                for parameter in &tool.params {
                                    if self.hir.parameter_injections.contains(parameter) {
                                        dependencies.insert(EffectUnit::ParameterDefault {
                                            owner: item,
                                            parameter: *parameter,
                                            expr: self.hir.parameter_defaults[parameter],
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            dependencies.into_iter().collect()
        })();
        dependencies.extend(self.omitted_argument_units(expr));
        dependencies
    }

    fn direct_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        callee: Self::Unit,
        summary: &Self::Summary,
        mut state: Self::Domain,
    ) -> Self::Domain {
        self.record_agent_dispatch(site.call, &mut state);
        let frozen = if let EffectUnit::Item(item) = callee {
            self.frozen_calls.get(&item).cloned()
        } else {
            None
        };
        let summary = match &frozen {
            Some(crate::FrozenCallableSummary::Solved(summary))
            | Some(crate::FrozenCallableSummary::Replaced { pinned: summary }) => summary.as_ref(),
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
        let invoked_projections = Self::invoked_parameter_projections(summary);
        let mut summary =
            match self.specialize_summary_for_call(callee, &site, &state, summary, None) {
                Ok(summary) => summary,
                Err(super::specialize::EffectSpecializationError::Interrupted(interruption)) => {
                    self.callable_summaries.borrow_mut().interruption = Some(interruption);
                    return state;
                }
                Err(error) => {
                    return self.incomplete_at(
                        site.span,
                        format!("call effect selector specialization failed: {error}"),
                        state,
                    );
                }
            };
        self.specialize_filesystem_open_call(&site, &mut summary);
        if let Err(error) =
            self.record_source_call_arguments(callee, &site, &state, None, &invoked_projections)
        {
            return self.incomplete_at(site.span, error.to_string(), state);
        }
        if let Err(error) = self
            .apply_agent_helper_state(&site, callee, &summary, &mut state)
            .and_then(|()| self.apply_committed_agent_state(&site, callee, &summary, &mut state))
        {
            return self.incomplete_at(site.span, error.to_string(), state);
        }
        self.note_call_outcomes(site.call, Some(callee), &summary, &mut state);
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
                let invocation_source = if matches!(
                    source,
                    LatentSource::Parameter { .. } | LatentSource::Projection(_)
                ) && matches!(self.hir.exprs.get(site.call), Some(etas_hir::HirExpr::Call { args, .. }) if !args.is_empty())
                {
                    let inputs = match self.call_result_step(site.call, &state) {
                        Ok(etas_code::descriptor::ValueProjectionStep::CallWithArguments {
                            inputs,
                        }) => inputs,
                        Ok(_) => unreachable!("checked application records its inputs"),
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                    };
                    match self
                        .project_parameter_origins(
                            [source],
                            &[etas_code::descriptor::ValueProjectionStep::Invocation { inputs }],
                        )
                        .and_then(|origins| origins.into_iter().next())
                    {
                        Some(source) => source,
                        None => {
                            return self.incomplete_at(
                                site.span,
                                "callback invocation has no checked input projection",
                                state,
                            );
                        }
                    }
                } else {
                    source
                };
                if invocation_source != source {
                    let recorded = self
                        .inputs
                        .dynamic_call_sources
                        .entry(site.call)
                        .or_default();
                    recorded.remove(&source);
                    recorded.insert(invocation_source);
                }
                let mut summary = match self.latent_source_summary(invocation_source, site.span) {
                    Ok(summary) => summary,
                    Err(error) => {
                        return self.incomplete_at(site.span, error.to_string(), state);
                    }
                };
                if let LatentSource::ExternalFrozen { reflection } = source {
                    let Some(metadata) = self.external_reflection_metadata(reflection) else {
                        return self.incomplete_at(
                            site.span,
                            "external frozen callable has no unique checked metadata",
                            state,
                        );
                    };
                    summary = match self.external_call_summary_from_metadata(
                        metadata,
                        &site,
                        &mut state,
                        Some(reflection),
                    ) {
                        Ok(Some(summary)) => summary,
                        Ok(None) => {
                            return self.incomplete_at(
                                site.span,
                                "external frozen callable has incomplete checked effect facts",
                                state,
                            );
                        }
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                    };
                } else if let LatentSource::ExternalReference { value, .. } = source {
                    let Some(metadata) = self.external_reference_metadata(value) else {
                        return self.incomplete_at(
                            site.span,
                            "external callable reference has no unique checked metadata",
                            state,
                        );
                    };
                    summary = match self.external_call_summary_from_metadata(
                        metadata,
                        &site,
                        &mut state,
                        Some(value),
                    ) {
                        Ok(Some(summary)) => summary,
                        Ok(None) => {
                            return self.incomplete_at(
                                site.span,
                                "external callable reference has incomplete checked effect facts",
                                state,
                            );
                        }
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                    };
                } else if let LatentSource::Metadata(source) = source {
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
                } else if let LatentSource::FrozenContract { unit, .. }
                | LatentSource::Unit(unit)
                | LatentSource::Reference { unit, .. }
                | LatentSource::Captured { unit, .. } = source
                {
                    let environment = match source {
                        LatentSource::FrozenContract {
                            unit,
                            reference: Some(value),
                            environment,
                        } => {
                            match self.reference_invocation_environment(LatentSource::Reference {
                                unit,
                                value,
                                environment,
                            }) {
                                Ok(environment) => environment,
                                Err(error) => {
                                    return self.incomplete_at(site.span, error.to_string(), state);
                                }
                            }
                        }
                        LatentSource::FrozenContract {
                            reference: None,
                            environment,
                            ..
                        } => environment,
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
                    let invoked_projections = Self::invoked_parameter_projections(&summary);
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
                    if let Err(error) = self.record_source_call_arguments(
                        unit,
                        &site,
                        &state,
                        environment,
                        &invoked_projections,
                    ) {
                        return self.incomplete_at(site.span, error.to_string(), state);
                    }
                    state = self.validate_returned_flow_source(&site, state);
                } else if matches!(
                    source,
                    LatentSource::Parameter { .. } | LatentSource::Projection(_)
                ) && (returns_flow || self.expr_type_has_parameters(site.call))
                {
                    let step = match self.call_result_step(site.call, &state) {
                        Ok(step) => step,
                        Err(error) => {
                            return self.incomplete_at(site.span, error.to_string(), state);
                        }
                    };
                    let Some(origins) = self.project_parameter_origins([source], &[step]) else {
                        return self.incomplete_at(
                            site.span,
                            format!(
                                "callback result requires its checked value projection: {source:?}"
                            ),
                            state,
                        );
                    };
                    state
                        .local_latent_values
                        .record_value_sources(site.call, origins);
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
                    // Exit facts identify the callable; their argument field
                    // already records this invocation's checked inputs. The
                    // invocation projection is retained in the effect trace.
                    if let Some(target) = match source {
                        LatentSource::Parameter { symbol, .. } => {
                            Some(crate::ParameterCallTarget::Native(symbol))
                        }
                        LatentSource::Projection(id) => {
                            Some(crate::ParameterCallTarget::Projection(id))
                        }
                        _ => None,
                    } {
                        let Some(output) = self.types.facts.expr_types.get(&site.call).copied()
                        else {
                            return self.incomplete_at(
                                site.span,
                                "callback completion requires the checked call output type",
                                state,
                            );
                        };
                        parameter_calls.insert(crate::OutcomeAnalysisGap::ParameterCall {
                            target,
                            output,
                            source: site.span.into(),
                        });
                        let arguments = match self.hir.exprs.get(site.call) {
                            Some(etas_hir::HirExpr::Call { args, .. }) => args
                                .iter()
                                .map(|arg| {
                                    let (name, value) = match arg {
                                        etas_hir::HirArg::Positional(value) => (None, *value),
                                        etas_hir::HirArg::Named { name, value, .. } => {
                                            (Some(name.clone()), *value)
                                        }
                                    };
                                    let ty = *self.types.facts.expr_types.get(&value)?;
                                    let expression = self.hir.exprs.get(value)?;
                                    let checked_value =
                                        super::argument_values::checked_literal_value(
                                            self.hir, self.types, expression, ty,
                                        )
                                        .or_else(|| {
                                            self.argument_writes.input_value(
                                                self.hir,
                                                self.types,
                                                context.unit,
                                                expression,
                                                ty,
                                            )
                                        });
                                    Some(crate::ParameterExitArgument {
                                        name,
                                        ty,
                                        source: expression.span(&self.hir.blocks).into(),
                                        value: checked_value,
                                    })
                                })
                                .collect(),
                            _ => None,
                        };
                        let handler_context = match self.enclosing_handler_context(site.call) {
                            Ok(value) => value,
                            Err(interruption) => {
                                self.callable_summaries.borrow_mut().interruption =
                                    Some(interruption);
                                return state;
                            }
                        };
                        state.exits.record_parameter_exit(crate::ParameterExitFact {
                            target,
                            output,
                            source: site.span.into(),
                            arguments,
                            handler_free: handler_context.free,
                            enclosing_handled: handler_context.handled,
                            enclosing_handlers: handler_context.handlers,
                            prefix: state.summary.action_trace.clone(),
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
            if flow.declared_effects().is_none() && self.is_projected_flow_value(site.callee_expr) {
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
                Some(crate::FrozenCallableSummary::Solved(summary))
                | Some(crate::FrozenCallableSummary::Replaced { pinned: summary }) => {
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
