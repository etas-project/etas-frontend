use super::engine::EffectSemantics;
use super::shared::*;
use crate::{ActionEvent, pipeline::ExternalActionTraceMetadata};

impl EffectSemantics<'_> {
    pub(crate) fn source_import_target_missing(&self, callee: HirExprId) -> bool {
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
        path.first().is_none_or(|segment| segment != "std")
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
    ) -> Result<Option<EffectSummary>, super::specialize::EffectSpecializationError> {
        let Some(metadata) = self
            .external_summaries
            .iter()
            .find(|summary| external_summary_matches_path(summary, path))
            .cloned()
        else {
            return Ok(None);
        };
        let Some(mut summary) = self.external_summary_from_metadata(&metadata, site.span) else {
            return Ok(None);
        };
        let bindings = self.call_bindings_from_param_names(site, &metadata.param_names);
        let (type_bindings, mut effect_row_bindings, deferred_effect_row_obligations) = self
            .types
            .facts
            .generic_instantiations
            .get(&site.call)
            .map(|fact| {
                let deferred = fact
                    .deferred_effect_row_obligations
                    .iter()
                    .filter(|obligation| metadata.effect_param_names.contains(&obligation.param))
                    .cloned()
                    .collect::<Vec<_>>();
                (
                    fact.type_bindings
                        .iter()
                        .filter(|(name, _)| metadata.type_param_names.contains(name))
                        .cloned()
                        .collect::<Vec<_>>(),
                    fact.effect_row_bindings
                        .iter()
                        .filter(|(name, _)| metadata.effect_param_names.contains(name))
                        .cloned()
                        .collect::<Vec<_>>(),
                    deferred,
                )
            })
            .unwrap_or_default();
        effect_row_bindings.extend(deferred_effect_row_obligations.iter().map(|obligation| {
            (
                obligation.param.clone(),
                etas_types::EffectRowRef {
                    effects: Vec::new(),
                    tail: None,
                },
            )
        }));
        self.require_named_effect_type_bindings(
            &metadata.type_param_names,
            &summary,
            &type_bindings,
        )?;
        self.require_named_effect_row_bindings(
            &metadata.effect_param_names,
            &summary,
            &effect_row_bindings,
        )?;
        self.validate_deferred_effect_row_obligations(state, &deferred_effect_row_obligations)?;
        summary = self.specialize_summary_with_bindings(
            &summary,
            &bindings,
            &type_bindings,
            &effect_row_bindings,
        )?;
        let mut merged_params = BTreeSet::new();
        for obligation in &deferred_effect_row_obligations {
            if !merged_params.insert(obligation.param.clone()) {
                continue;
            }
            let mut source_summary =
                self.latent_summary_for_expr(state, obligation.source, &obligation.param)?;
            source_summary.action_trace = ActionTraceDomain::Empty;
            summary.seq_assign(&source_summary);
        }
        if self
            .apply_external_function_parameter_effects(
                &mut summary,
                site,
                state,
                &type_bindings,
                &effect_row_bindings,
                &deferred_effect_row_obligations,
            )?
            .is_none()
        {
            return Ok(None);
        }
        summary.action_trace =
            self.specialize_parameter_call_trace(&summary.action_trace, &bindings, state)?;
        Ok(Some(summary))
    }

    pub(crate) fn external_summary_from_metadata(
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
        summary.escaping_effects.open = metadata
            .public_effects
            .tail
            .as_deref()
            .map(effect_var_id_from_name);
        summary.requested_actions.open = metadata
            .requested_actions
            .tail
            .as_deref()
            .map(effect_var_id_from_name);
        summary.handled_actions.open = metadata
            .handled_requested_actions
            .tail
            .as_deref()
            .map(effect_var_id_from_name);
        summary.action_trace =
            self.external_action_trace_from_metadata(&metadata.action_trace, span)?;
        Some(summary)
    }

    fn external_action_trace_from_metadata(
        &self,
        trace: &ExternalActionTraceMetadata,
        span: Span,
    ) -> Option<ActionTraceDomain> {
        Some(match trace {
            ExternalActionTraceMetadata::Empty => ActionTraceDomain::Empty,
            ExternalActionTraceMetadata::Event { action, source } => {
                ActionTraceDomain::Event(ActionEvent {
                    action: self.external_effect_from_metadata(action)?,
                    span,
                    source: source.clone(),
                })
            }
            ExternalActionTraceMetadata::ParameterCall { parameter } => {
                ActionTraceDomain::ParameterCall {
                    parameter: parameter.clone(),
                    span,
                }
            }
            ExternalActionTraceMetadata::Seq(children) => ActionTraceDomain::Seq(
                children
                    .iter()
                    .map(|child| self.external_action_trace_from_metadata(child, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ExternalActionTraceMetadata::Choice(children) => ActionTraceDomain::Choice(
                children
                    .iter()
                    .map(|child| self.external_action_trace_from_metadata(child, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ExternalActionTraceMetadata::Repeat(child) => ActionTraceDomain::Repeat(Box::new(
                self.external_action_trace_from_metadata(child, span)?,
            )),
            ExternalActionTraceMetadata::UnknownOrder(actions) => {
                ActionTraceDomain::UnknownOrder(EffectSet::from_iter(
                    actions
                        .iter()
                        .map(|action| self.external_effect_from_metadata(action))
                        .collect::<Option<Vec<_>>>()?,
                ))
            }
        })
    }

    pub(crate) fn external_effect_from_metadata(
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

    pub(crate) fn external_effect_type_args(&self, args: &[EffectArgRef]) -> Option<Vec<TypeId>> {
        args.iter()
            .map(|arg| match arg {
                EffectArgRef::Type(ty) => Some(*ty),
                EffectArgRef::Path(path) => self.type_id_for_external_effect_path(path),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn record_external_summary_requested_action(
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

    pub(crate) fn apply_external_function_parameter_effects(
        &mut self,
        summary: &mut EffectSummary,
        site: &CallSite<EffectUnit>,
        state: &EffectState,
        type_bindings: &[(String, TypeId)],
        effect_row_bindings: &[(String, etas_types::EffectRowRef)],
        deferred_effect_row_obligations: &[etas_types::DeferredEffectRowObligation],
    ) -> Result<Option<()>, super::specialize::EffectSpecializationError> {
        let Some(callee_input) = self
            .flow_type_for_expr(site.callee_expr)
            .map(|flow| flow.input.clone())
        else {
            return Ok(Some(()));
        };
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(site.call) else {
            return Ok(Some(()));
        };
        for (index, param_ty) in callee_input.iter().enumerate() {
            let Some(arg) = args.get(index).and_then(arg_expr) else {
                continue;
            };
            let Some(Type::Function(flow)) = self.types.store.get(*param_ty) else {
                continue;
            };
            if let Some(row) = &flow.effects {
                if row.tail.as_ref().is_some_and(|tail| {
                    deferred_effect_row_obligations
                        .iter()
                        .any(|obligation| obligation.param == *tail && obligation.source == arg)
                }) {
                    continue;
                }
                let row = self.specialize_effect_row(
                    &self.row_from_type_ref(row),
                    &[],
                    type_bindings,
                    effect_row_bindings,
                )?;
                summary.seq_assign(&self.summary_for_invoked_flow_row(row, site.span));
                continue;
            }
            let Some(sources) = self.latent_sources_for_expr(state, arg) else {
                return Ok(None);
            };
            for source in sources {
                let Some(source_summary) = self.inputs.unit_effects.get(&source).cloned() else {
                    return Ok(None);
                };
                summary.seq_assign(&source_summary);
                self.record_latent_realization(source, site.call);
            }
        }
        Ok(Some(()))
    }
}
