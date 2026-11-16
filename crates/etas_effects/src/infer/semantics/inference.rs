use std::collections::BTreeSet;

use etas_hir::{HirAnnotationArg, HirItem, SymbolId};

use super::{
    engine::{EffectSemantics, annotation_name},
    shared::*,
};

pub(super) enum ModelToolSource {
    Item(HirItemId),
    External(Vec<String>),
}

impl EffectSemantics<'_> {
    fn tool_invocation_context(
        &self,
        expr: HirExprId,
    ) -> Result<Option<etas_types::TypeId>, String> {
        let Some(fact) = self
            .types
            .facts
            .std_intrinsic_calls
            .get(&expr)
            .filter(|fact| fact.intrinsic == etas_std::intrinsic::runtime::TOOL_INVOKE)
        else {
            return Ok(None);
        };
        let Some(etas_types::Type::Applied { args, .. }) =
            self.types.store.get(fact.signature.output)
        else {
            return Err("tool invocation has no checked ToolResult<C> type".into());
        };
        match args.as_slice() {
            [context] => Ok(Some(*context)),
            _ => Err("tool invocation has no single checked agent context".into()),
        }
    }

    pub(super) fn agent_context_parameter(
        &self,
        ty: etas_types::TypeId,
        owner: Option<HirItemId>,
    ) -> Option<&etas_types::CallableGenericParam> {
        let signature = self.types.facts.item_signatures.get(&owner?)?;
        let signature = match signature {
            etas_types::ItemSignature::Flow(signature)
            | etas_types::ItemSignature::Tool(signature) => signature,
            etas_types::ItemSignature::Agent(agent) => agent.callable()?,
            etas_types::ItemSignature::TopLevelLet(_) => return None,
        };
        signature.generic_params.iter().find(|param| {
            param.subject == ty && param.kind == etas_types::CallableGenericParamKind::Type
        })
    }

    pub(super) fn tool_invocation_agent(
        &self,
        expr: HirExprId,
    ) -> Result<Option<HirItemId>, String> {
        let Some(context) = self.tool_invocation_context(expr)? else {
            return Ok(None);
        };
        let Some(etas_types::Type::AgentContext(etas_types::AgentContextRef::Source(method))) =
            self.types.store.get(context)
        else {
            return Ok(None);
        };
        self.types
            .facts
            .agent_methods
            .get(method)
            .map(|method| Some(method.agent))
            .ok_or_else(|| "tool invocation context has no checked method".into())
    }

    pub(super) fn agent_tool_summary(
        &self,
        context: etas_types::TypeId,
        span: Span,
    ) -> Result<EffectSummary, String> {
        let Some(etas_types::Type::AgentContext(context)) = self.types.store.get(context) else {
            return Err("tool invocation requires a checked agent context".into());
        };
        let method = match context {
            etas_types::AgentContextRef::Source(method) => method,
            etas_types::AgentContextRef::Portable(method) => {
                return self.portable_agent_tool_summary(method, span);
            }
        };
        let agent = self
            .types
            .facts
            .agent_methods
            .get(method)
            .ok_or("tool invocation context has no checked method")?
            .agent;
        self.agent_tool_choices(agent, span)
    }

    fn portable_agent_tool_summary(
        &self,
        method: &etas_code::MethodId,
        span: Span,
    ) -> Result<EffectSummary, String> {
        use etas_types::facts::reflection::ExternalAgentReflectionFact;
        let header = self
            .types
            .facts
            .external_agent_reflections
            .values()
            .find_map(|fact| {
                let ExternalAgentReflectionFact::Available { header, .. } = fact else {
                    return None;
                };
                (header.identity == *method.implementation().subject()
                    && header.method_info.as_ref().is_some_and(|methods| {
                        methods
                            .iter()
                            .any(|entry| entry.callable.as_method() == Some(method))
                    }))
                .then_some(header)
            })
            .ok_or("portable agent context has no checked method/tool surface")?;
        let mut choices = EffectSummary::local();
        for tool in &header.tools {
            let callable = etas_code::CallableId::declaration(tool.clone())
                .map_err(|error| error.to_string())?;
            let metadata = self
                .external_metadata_for_callable(&callable)
                .ok_or("exposed external tool has no canonical effect summary")?;
            let summary = self
                .external_summary_from_metadata(metadata, span)
                .ok_or("exposed external tool effect summary is incomplete")?;
            choices.join_branch(&summary);
        }
        Ok(choices)
    }

    pub(super) fn tool_invocation_summary(
        &self,
        expr: HirExprId,
        span: Span,
        state: &EffectState,
    ) -> Result<EffectSummary, String> {
        let context = self
            .tool_invocation_context(expr)?
            .ok_or("call is not a checked tool invocation")?;
        if let Some(param) = self.agent_context_parameter(context, state.owner) {
            let target = crate::ParameterCallTarget::AgentTools(context);
            let mut summary = EffectSummary::local();
            summary.action_trace = ActionTraceDomain::ParameterCall {
                target,
                parameter: param.name.clone(),
                span,
            };
            summary.handler_requests =
                crate::RequestInstantiationSummary::parameter(target, param.name.clone(), span);
            return Ok(summary);
        }
        self.agent_tool_summary(context, span)
    }

    pub(super) fn bind_agent_tool_arguments(
        &self,
        summary: &EffectSummary,
        bindings: &mut super::specialize::TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<(), super::specialize::EffectSpecializationError> {
        let mut calls = summary.action_trace.parameter_calls();
        if let crate::RequestInstantiationSummary::Available { parameters, .. } =
            &summary.handler_requests
        {
            calls.extend(parameters.iter().map(|parameter| {
                crate::infer::domain::ParameterCallRef {
                    target: parameter.target,
                    parameter: parameter.parameter.clone(),
                    span: parameter.span,
                }
            }));
        }
        for call in calls {
            let crate::ParameterCallTarget::AgentTools(context) = call.target else {
                continue;
            };
            if matches!(
                self.types.store.get(context),
                Some(etas_types::Type::AgentContext(_))
            ) {
                bindings.insert(
                    call.target,
                    super::specialize::TraceArgumentBinding::AgentTools(context),
                );
            } else if self.agent_context_parameter(context, state.owner).is_none() {
                return Err(
                    super::specialize::EffectSpecializationError::MissingTypeBinding {
                        param: call.parameter,
                    },
                );
            }
        }
        Ok(())
    }

    fn agent_tool_choices(&self, agent: HirItemId, span: Span) -> Result<EffectSummary, String> {
        let mut choices = EffectSummary::local();
        for tool in self
            .agent_model_tools(agent)
            .ok_or("agent tool surface is missing checked facts")?
        {
            let tool = match tool {
                ModelToolSource::Item(item) => self
                    .latent_source_summary(
                        super::shared::LatentSource::Unit(EffectUnit::Item(item)),
                        span,
                    )
                    .map_err(|error| error.to_string())?,
                ModelToolSource::External(path) => self
                    .external_summary_for_path(&path, span)
                    .ok_or("exposed external tool has no checked effect summary")?,
            };
            choices.join_branch(&tool);
        }
        Ok(choices)
    }
    pub(super) fn agent_model_tools(&self, agent: HirItemId) -> Option<Vec<ModelToolSource>> {
        let mut tools = Vec::new();
        let mut symbols = BTreeSet::new();
        for annotation in self.hir.item_annotations.get(&agent).into_iter().flatten() {
            if annotation_name(annotation) != "tools" {
                continue;
            }
            for arg in &annotation.args {
                let value = match arg {
                    HirAnnotationArg::Positional { value, .. } => *value,
                    HirAnnotationArg::Named { name, value, .. }
                        if name == "tools" || name == "items" =>
                    {
                        *value
                    }
                    HirAnnotationArg::Named { name, .. } if name == "choice" || name == "mode" => {
                        continue;
                    }
                    _ => return None,
                };
                let entries = match self.hir.exprs.get(value)? {
                    HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems.as_slice(),
                    _ => std::slice::from_ref(&value),
                };
                for entry in entries {
                    let symbol = self.callee_symbol(*entry)?;
                    if symbols.insert(symbol) {
                        tools.push(self.model_tool_source(symbol)?);
                    }
                }
            }
        }
        Some(tools)
    }

    fn model_tool_source(&self, symbol: SymbolId) -> Option<ModelToolSource> {
        let binding = self.hir.symbols.get(symbol)?;
        match &binding.def {
            SymbolDef::Item { item }
                if matches!(self.hir.items.get(*item), Some(HirItem::Tool(_))) =>
            {
                Some(ModelToolSource::Item(*item))
            }
            SymbolDef::ImportAlias { path, .. }
                if matches!(
                    self.types.facts.symbol_types.get(&symbol),
                    Some(etas_types::SymbolTypeFact::Tool { .. })
                ) =>
            {
                Some(ModelToolSource::External(path.clone()))
            }
            _ => None,
        }
    }

    pub(crate) fn agent_infer_summary(
        &self,
        expr: HirExprId,
        fact: &etas_types::facts::inference::CheckedAgentInferenceFact,
        action: &ResolvedActionRef,
        span: Span,
    ) -> Option<(Effect, EffectSummary)> {
        let method = self.types.facts.agent_methods.get(&fact.method)?;
        if method.agent != fact.agent || method.body != fact.owner {
            return None;
        }
        let Effect::Action(action) = self.effect_from_action_ref(action)? else {
            return None;
        };
        let controller = self.types.facts.agent_controllers.get(&expr)?;
        let mut summary = EffectSummary::local();
        let effect = Effect::AppliedAction(ActionInstanceRef {
            action: action.clone(),
            args: vec![
                EffectArgRef::Type(controller.context),
                EffectArgRef::Type(fact.output),
            ],
        });
        summary.record_requested_action(effect.clone());
        summary.record_default_handled_action(effect.clone());
        summary.record_action_trace_event(effect.clone(), span, crate::ActionEventSource::Perform);
        let tool_choices = self.agent_tool_choices(fact.agent, span).ok()?;
        let descriptor = self.std_registry.intrinsic(etas_std::StdIntrinsicId(
            etas_std::intrinsic::runtime::AGENT_STEP,
        ))?;
        let etas_std::LoweringHint::DefaultAction { stable_id, .. } = &descriptor.lowering else {
            return None;
        };
        let signature = self
            .registry
            .standard_action(crate::EffectActionId(*stable_id))?;
        let step = Effect::AppliedAction(ActionInstanceRef {
            action: crate::ActionRef {
                tag: signature.owner,
                action: signature.id,
            },
            args: vec![
                EffectArgRef::Type(controller.context),
                EffectArgRef::Type(fact.output),
            ],
        });
        let mut iteration = EffectSummary::local();
        if let Some(reason) = self
            .registry
            .runtime_requirement_reason_for_action_ref(&action)
        {
            iteration.require_runtime(reason);
        }
        iteration.record_requested_action(step.clone());
        iteration.record_default_handled_action(step.clone());
        iteration
            .record_default_handler_effects(step.clone(), EffectRow::empty())
            .ok()?;
        iteration.record_action_trace_event(step, span, crate::ActionEventSource::Perform);
        iteration.seq_assign(&tool_choices);
        summary
            .record_default_behavior(effect.clone(), &iteration.repeated())
            .ok()?;
        Some((effect, summary))
    }
}
