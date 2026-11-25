use super::{engine::EffectSemantics, state::EffectState};
use crate::{ActionInstanceRef, Effect, EffectSummary};
use etas_hir::{HirExprId, HirItem, HirItemId, HirReceiverMode, SymbolId};
use etas_types::TypeId;

impl EffectSemantics<'_> {
    pub(super) fn agent_receiver_parameter(&self, owner: HirItemId) -> Option<(SymbolId, TypeId)> {
        let receiver = match self.hir.items.get(owner)? {
            HirItem::Flow(flow) if flow.receiver_mode != HirReceiverMode::None => {
                let method = self.types.facts.agent_methods.get(&flow.symbol)?;
                self.types.facts.agent_signature(method.component)?.state?;
                *flow.params.first()?
            }
            HirItem::Tool(_) => self.hir.member_tool_receiver(owner)?,
            _ => return None,
        };
        let ty = self.symbol_value_type(receiver)?;
        let layout = self
            .types
            .facts
            .agent_state_type(&self.types.store, ty)
            .ok()??;
        Some((receiver, layout))
    }

    pub(super) fn record_agent_receiver_projections(
        &self,
        owner: HirItemId,
        state: &mut EffectState,
    ) -> Result<(), super::specialize::EffectSpecializationError> {
        use super::{specialize::EffectSpecializationError, state::LatentSource};
        use std::collections::BTreeSet;

        let Some((receiver, layout)) = self.agent_receiver_parameter(owner) else {
            return Ok(());
        };
        let paths = self.value_payload_paths(layout).ok_or_else(|| {
            EffectSpecializationError::InvalidCallableEnvironment {
                reason: "Agent state has no checked callable payload paths".into(),
            }
        })?;
        for path in paths {
            let projection = self.intern_parameter_projection(owner, receiver, path.clone())?;
            state.local_latent_values.symbol_projections.insert(
                (receiver, path),
                BTreeSet::from([LatentSource::Projection(projection)]),
            );
        }
        Ok(())
    }

    fn updating_agent_receiver(&self, owner: HirItemId) -> Option<(SymbolId, TypeId)> {
        let HirItem::Flow(flow) = self.hir.items.get(owner)? else {
            return None;
        };
        (flow.receiver_mode == HirReceiverMode::Updating)
            .then(|| self.agent_receiver_parameter(owner))
            .flatten()
    }

    pub(super) fn capture_agent_state_outputs(
        &self,
        unit: crate::EffectUnit,
        state: &EffectState,
        result: &mut crate::CallableExitSummary,
    ) {
        use crate::ProjectedReturnSources;
        let crate::EffectUnit::Item(owner) = unit else {
            return;
        };
        let Some((receiver, _)) = self.updating_agent_receiver(owner) else {
            return;
        };
        let mut exit = crate::CallableExitSummary::default();
        for ((symbol, path), origins) in &state.local_latent_values.symbol_projections {
            if *symbol != receiver {
                continue;
            }
            let origins = origins.iter().copied().collect::<Vec<_>>();
            exit.receiver_projections.insert(
                path.clone(),
                self.returned_callable_sources(state, &origins).map_or(
                    ProjectedReturnSources::Unavailable,
                    ProjectedReturnSources::Known,
                ),
            );
        }
        result.join_assign(&exit);
    }

    pub(super) fn apply_agent_helper_state(
        &self,
        site: &etas_hir_analysis::interprocedural::CallSite<crate::EffectUnit>,
        callee: crate::EffectUnit,
        summary: &EffectSummary,
        state: &mut EffectState,
    ) -> Result<(), super::specialize::EffectSpecializationError> {
        use etas_types::facts::methods::MethodCallKind;
        let Some(method) = self.types.facts.checked_method_calls.get(&site.call) else {
            return Ok(());
        };
        if method.kind != MethodCallKind::AgentTurnHelper || !method.receiver.is_updating() {
            return Ok(());
        }
        let Some((receiver, layout)) = state
            .owner
            .and_then(|owner| self.updating_agent_receiver(owner))
        else {
            return Ok(());
        };
        let Some(paths) = self.value_payload_paths(layout) else {
            return Err(super::specialize::EffectSpecializationError::InvalidCallableReturnSources);
        };
        let updates =
            self.agent_state_at_return(site, callee, summary, state, paths.into_iter().collect())?;
        for (path, sources) in updates {
            state
                .local_latent_values
                .symbol_projections
                .insert((receiver, path), sources);
        }
        Ok(())
    }

    pub(super) fn agent_state_at_return(
        &self,
        site: &etas_hir_analysis::interprocedural::CallSite<crate::EffectUnit>,
        callee: crate::EffectUnit,
        summary: &EffectSummary,
        state: &EffectState,
        paths: Vec<Vec<etas_code::descriptor::ValueProjectionStep>>,
    ) -> Result<
        Vec<(
            Vec<etas_code::descriptor::ValueProjectionStep>,
            std::collections::BTreeSet<super::state::LatentSource>,
        )>,
        super::specialize::EffectSpecializationError,
    > {
        use crate::{ParameterCallTarget, ProjectedReturnSources};
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let params = self.invocation_params(callee, None)?.ok_or(
            super::specialize::EffectSpecializationError::InvalidCallableParameterInterface,
        )?;
        let targets = params
            .into_iter()
            .map(ParameterCallTarget::Native)
            .collect::<Vec<_>>();
        let bindings = self.call_trace_bindings(site, &targets, state)?;
        let proven = summary.exits.as_ref().filter(|exits| exits.returns_only());
        let mut updates = Vec::new();
        for path in paths {
            let sources = match proven.and_then(|exits| exits.receiver_projections.get(&path)) {
                Some(ProjectedReturnSources::Known(sources)) => {
                    self.returned_callable_bindings(sources, &bindings, state, site.call, None)?
                }
                // A helper changes the pending turn directly. Caught errors
                // may leave partial writes, so success-only evidence cannot
                // stand in for the state after an unproved helper exit.
                _ => vec![super::state::LatentSource::Unavailable { expr: site.call }],
            };
            updates.push((path, sources.into_iter().collect()));
        }
        Ok(updates)
    }

    pub(super) fn stable_agent_state_layout(&self, ty: TypeId) -> Option<TypeId> {
        let component = match self.types.store.get(ty)? {
            etas_types::Type::Applied { constructor, .. } => TypeId(constructor.0),
            _ => ty,
        };
        let layout = self
            .types
            .facts
            .agent_state_type(&self.types.store, ty)
            .ok()??;
        // A receiver argument is a reference. Only reuse constructor origins
        // when the instance state cannot be replaced by a later request.
        let can_update = self.types.facts.agent_methods.values().any(|method| {
            method.component == component
                && matches!(self.hir.items.get(method.body),
                Some(HirItem::Flow(flow)) if flow.receiver_mode == HirReceiverMode::Updating)
        });
        (!can_update).then_some(layout)
    }

    pub(super) fn managed_agent_boundary(
        &self,
        name: &str,
        selector: TypeId,
        span: etas_core::Span,
    ) -> Option<EffectSummary> {
        let action = self.registry.action_by_name(&format!("Agentic.{name}"))?;
        let mut summary = EffectSummary::local();
        summary.record_managed_agent_boundary(
            Effect::AppliedAction(ActionInstanceRef {
                action,
                args: vec![etas_types::EffectArgRef::Type(selector)],
            }),
            span,
        );
        Some(summary)
    }

    pub(super) fn record_agent_dispatch(&mut self, expr: HirExprId, state: &mut EffectState) {
        let Some(fact) = self.types.facts.checked_method_calls.get(&expr) else {
            return;
        };
        if fact.kind != etas_types::facts::methods::MethodCallKind::AgentRequest
            || matches!(
                fact.target,
                etas_types::method::MethodTarget::BoundSpec { .. }
            )
        {
            return;
        }
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        let Some(summary) = fact
            .request_context
            .and_then(|context| self.managed_agent_boundary("call", context, span))
        else {
            self.mark_incomplete_at(
                span,
                "managed request has no checked method/action identity",
                state,
            );
            return;
        };
        state.summary.seq_assign(&summary);
    }

    pub(super) fn is_bound_agent_request(&self, call: HirExprId) -> bool {
        use etas_types::{facts::methods::MethodCallKind, method::MethodTarget};
        self.types
            .facts
            .checked_method_calls
            .get(&call)
            .is_some_and(|fact| {
                fact.kind == MethodCallKind::AgentRequest
                    && matches!(fact.target, MethodTarget::BoundSpec { .. })
            })
    }

    pub(super) fn agent_request_summary(
        &self,
        source: super::state::LatentSource,
        span: etas_core::Span,
        mut body: EffectSummary,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        use super::{specialize::EffectSpecializationError, state::LatentSource};
        use crate::EffectUnit;

        let context = match source {
            LatentSource::Captured {
                unit: EffectUnit::Item(item),
                ..
            }
            | LatentSource::Unit(EffectUnit::Item(item)) => self
                .hir
                .items
                .get(item)
                .and_then(etas_hir::HirItem::symbol)
                .and_then(|symbol| self.types.facts.agent_methods.get(&symbol))
                .map(|method| method.context_type),
            LatentSource::Metadata(source) => self
                .implementation_graphs
                .get(&source.graph)
                .and_then(|index| index.and_then(|index| self.external_implementations.get(index)))
                .and_then(|method| self.portable_agent_context(&method.identity)),
            _ => None,
        };
        let mut summary = context
            .and_then(|context| self.managed_agent_boundary("call", context, span))
            .ok_or_else(|| EffectSpecializationError::InvalidCallableEnvironment {
                reason: "managed request witness has no checked Agent method identity".into(),
            })?;
        if let Some(exits) = &mut body.exits {
            for exit in &mut exits.exits {
                let mut trace = summary.action_trace.clone();
                trace.seq_assign(std::mem::take(&mut exit.trace));
                exit.trace = trace;
            }
            for exit in &mut exits.parameter_exits {
                let mut trace = summary.action_trace.clone();
                trace.seq_assign(std::mem::take(&mut exit.prefix));
                exit.prefix = trace;
            }
        }
        summary.seq_assign(&body);
        summary.exits = body.exits;
        Ok(summary)
    }

    fn portable_agent_context(&self, identity: &etas_code::MethodId) -> Option<TypeId> {
        self.types.store.iter().find_map(|(ty, value)| {
            matches!(value,
                etas_types::Type::AgentContext(etas_types::AgentContextRef::Portable(method))
                    if method.as_ref() == identity
            )
            .then_some(ty)
        })
    }

    pub(super) fn record_agent_construction(&mut self, expr: HirExprId, state: &mut EffectState) {
        if !matches!(self.hir.exprs.get(expr), Some(etas_hir::HirExpr::Record(_))) {
            return;
        }
        let Some(ty) = self.types.facts.expression_producer_type(expr) else {
            return;
        };
        if !self.types.facts.is_active_agent(&self.types.store, ty) {
            return;
        }
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        let Some(summary) = self.managed_agent_boundary("create", ty, span) else {
            self.mark_incomplete_at(
                span,
                "managed construction has no checked Agentic.create identity",
                state,
            );
            return;
        };
        state.summary.seq_assign(&summary);
    }
}

impl EffectSemantics<'_> {
    pub(super) fn autonomous_entry(
        &self,
        expr: HirExprId,
    ) -> Result<Option<etas_types::method::MethodWitness<'_>>, String> {
        if !matches!(self.hir.exprs.get(expr), Some(etas_hir::HirExpr::Record(_))) {
            return Ok(None);
        }
        let Some(ty) = self.types.facts.expression_producer_type(expr) else {
            return Ok(None);
        };
        if !etas_types::active_agent::has_main(&self.types.facts, &self.types.store, ty) {
            return Ok(None);
        }
        etas_types::active_agent::autonomous_main(
            &self.types.facts,
            &self.types.store,
            &self.spec_facts,
            ty,
        )
    }

    pub(super) fn record_autonomous_start(&mut self, expr: HirExprId, state: &mut EffectState) {
        let selected = match self.autonomous_entry(expr) {
            Ok(Some(witness)) => (witness.declaration.clone(), witness.owner_bindings),
            Ok(None) => return,
            Err(reason) => {
                self.mark_incomplete_at(self.hir.exprs[expr].span(&self.hir.blocks), reason, state);
                return;
            }
        };
        self.record_structural_latent_result(expr, state);
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        match self.autonomous_summary(expr, state, selected.0, selected.1) {
            Ok(mut summary) => {
                summary.exits = None;
                summary.action_trace = crate::ActionTraceDomain::TaskStart {
                    task_site: expr,
                    body: Box::new(summary.action_trace),
                };
                state.summary.seq_assign(&summary);
            }
            Err(error) => self.mark_incomplete_at(span, error.to_string(), state),
        }
    }

    fn autonomous_summary(
        &self,
        expr: HirExprId,
        state: &EffectState,
        method: etas_types::method::MethodSignatureFact,
        owner_bindings: std::collections::HashMap<TypeId, TypeId>,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        use super::specialize::{EffectSpecializationError as Error, TraceArgumentBinding};
        use crate::facts::{CallableEnvironment, CallableEnvironmentId};
        use etas_types::method::MethodTarget;
        let invalid = |reason: &str| Error::InvalidCallableEnvironment {
            reason: reason.into(),
        };
        let owner = state
            .owner
            .ok_or_else(|| invalid("autonomous construction has no checked owner"))?;
        let site = self.callable_environment_site(expr)?;
        let snapshot = CallableEnvironment {
            owner,
            invocation_origin: None,
            invocation_inputs: Default::default(),
            generics: self.lexical_generic_values(owner)?,
            parameters: [(
                crate::ParameterCallTarget::Interface(0),
                self.trace_argument_origins(&TraceArgumentBinding::Expression(expr), state)?,
            )]
            .into(),
        };
        let invocation = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&snapshot.generics),
            instance: None,
            scope: Some(self.callable_environment_scope(owner)?),
            origin: site,
            application: site,
        };
        let (source, context) = match &method.target {
            MethodTarget::Source {
                callable, symbol, ..
            }
            | MethodTarget::Agent { callable, symbol } => {
                let context = self
                    .types
                    .facts
                    .agent_methods
                    .get(symbol)
                    .ok_or_else(|| invalid("autonomous main has no checked Agent method context"))?
                    .context_type;
                (
                    self.capture_source_method_implementation(
                        *callable,
                        invocation,
                        snapshot,
                        super::spec_methods::MethodInstantiation {
                            generics: etas_types::GenericInstantiationFact {
                                type_bindings: Vec::new(),
                                effect_row_bindings: Vec::new(),
                                deferred_effect_row_obligations: Vec::new(),
                            },
                            owner_bindings,
                        },
                    )?,
                    context,
                )
            }
            MethodTarget::External { identity } => {
                let context = self.portable_agent_context(identity).ok_or_else(|| {
                    invalid("external autonomous main has no checked method context")
                })?;
                (
                    self.capture_external_method(
                        identity,
                        &method.signature,
                        invocation,
                        snapshot,
                        &owner_bindings,
                        (Vec::new(), Vec::new()),
                    )?,
                    context,
                )
            }
            _ => {
                return Err(invalid(
                    "autonomous main does not have an implementation witness",
                ));
            }
        };
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        let mut summary = self
            .managed_agent_boundary("call", context, span)
            .ok_or_else(|| invalid("autonomous main has no checked Agentic.call identity"))?;
        summary.seq_assign(&self.latent_source_summary(source, span)?);
        Ok(summary)
    }
}
