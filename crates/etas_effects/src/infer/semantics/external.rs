use super::engine::EffectSemantics;
use super::shared::*;

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
    ) -> Result<Option<EffectSummary>, etas_hir_analysis::static_string::StaticStringEvaluationError>
    {
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
        summary = self.specialize_summary_with_bindings(&summary, &bindings, &[])?;
        if self
            .apply_external_function_parameter_effects(&mut summary, site, state)
            .is_none()
        {
            return Ok(None);
        }
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
        Some(summary)
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
            let sources = self.latent_sources_for_expr(state, arg)?;
            for source in sources {
                let source_summary = self.inputs.unit_effects.get(&source)?.clone();
                summary.seq_assign(&source_summary);
                self.record_latent_realization(source, site.call);
            }
        }
        Some(())
    }
}
