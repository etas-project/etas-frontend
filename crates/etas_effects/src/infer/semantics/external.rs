use super::engine::EffectSemantics;
use super::shared::*;
use crate::{ActionEvent, infer::domain::ParameterCallRef, pipeline::ExternalActionTraceMetadata};

fn parameter_target(
    parameters: &[(String, crate::ParameterCallTarget)],
    label: &str,
) -> Option<crate::ParameterCallTarget> {
    let mut matches = parameters.iter().filter(|(name, _)| name == label);
    match (matches.next(), matches.next()) {
        (Some((_, target)), None) => Some(*target),
        _ => None,
    }
}

impl EffectSemantics<'_> {
    pub(crate) fn source_import_target_missing(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(binding) = self.hir.symbols.get(symbol) else {
            return false;
        };
        let etas_hir::SymbolDef::ImportAlias {
            origin: etas_hir::ImportAliasOrigin::SourceImport,
            ..
        } = &binding.def
        else {
            return false;
        };
        let Some(canonical) = self.hir.canonical_source_symbol(symbol) else {
            return true;
        };
        if self.source_item_for_symbol(canonical).is_some() {
            return false;
        }
        let Some(etas_hir::SymbolDef::ImportAlias { path, .. }) =
            self.hir.symbols.get(canonical).map(|symbol| &symbol.def)
        else {
            return true;
        };
        path.first().is_none_or(|segment| segment != "std")
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
        &self,
        path: &[String],
        site: &CallSite<EffectUnit>,
        state: &mut EffectState,
    ) -> Result<Option<EffectSummary>, super::specialize::EffectSpecializationError> {
        let Some(metadata) = self
            .external_summaries
            .iter()
            .find(|summary| external_summary_matches_path(summary, path))
        else {
            return Ok(None);
        };
        let Some(mut summary) = self.external_summary_from_metadata(metadata, site.span) else {
            return Ok(None);
        };
        let bindings = self.call_bindings_from_param_names(site, &metadata.param_names);
        let parameter_targets = (0..metadata.param_names.len())
            .map(|position| crate::ParameterCallTarget::Interface(position as u32))
            .collect::<Vec<_>>();
        let trace_bindings = self.call_trace_bindings(site, &parameter_targets)?;
        let (type_bindings, effect_row_bindings) =
            self.checked_call_generic_values(site.call, state)?;
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
        let parameter_trace = summary.action_trace.clone();
        summary = self.specialize_summary_with_bindings(
            &summary,
            &bindings,
            &type_bindings,
            &effect_row_bindings,
        )?;
        let signature = self
            .checked_callable_type_for_expr(site.callee_expr)
            .ok_or(
                super::specialize::EffectSpecializationError::InvalidCallableParameterInterface,
            )?;
        let inferred_parameters =
            self.inferred_callback_parameters(&parameter_targets, &signature.input)?;
        self.merge_parameter_call_summaries(
            &mut summary,
            &parameter_trace,
            &trace_bindings,
            state,
            &inferred_parameters,
        )?;
        summary.action_trace =
            self.specialize_parameter_call_trace(&summary.action_trace, &trace_bindings, state)?;
        if let Some(graph) = &metadata.returned_callables {
            let arguments = self.call_arguments(site, metadata.param_names.len())?;
            self.record_metadata_sources(
                super::metadata_callable::MetadataCallBindings {
                    key: metadata.callable_graph_key,
                    scope: etas_package_metadata::CallableValueScope::Root,
                    environment: None,
                    arguments: &arguments,
                    generics: &metadata.generic_params,
                    type_bindings: &type_bindings,
                    effect_row_bindings: &effect_row_bindings,
                },
                &graph.sources,
                state,
                site.call,
            )?;
        }
        Ok(Some(summary))
    }

    pub(crate) fn external_summary_from_metadata(
        &self,
        metadata: &crate::ExternalEffectSummaryMetadata,
        span: Span,
    ) -> Option<EffectSummary> {
        let parameter_targets = metadata
            .param_names
            .iter()
            .enumerate()
            .map(|(position, name)| {
                (
                    name.clone(),
                    crate::ParameterCallTarget::Interface(position as u32),
                )
            })
            .collect::<Vec<_>>();
        let mut summary = self.external_callable_summary(
            crate::pipeline::ExternalCallableEffectView {
                handler_requests: metadata.handler_requests.as_ref(),
                parameter_targets: &parameter_targets,
                public_effects: &metadata.public_effects,
                requested_actions: &metadata.requested_actions,
                handled_requested_actions: &metadata.handled_requested_actions,
                default_handler_effects: metadata.default_handler_effects.as_ref(),
                action_trace: &metadata.action_trace,
            },
            span,
        )?;
        if let Some(behavior) = &metadata.behavior {
            if !super::metadata_callable::apply_metadata_behavior(&mut summary, behavior) {
                return None;
            }
        }
        Some(summary)
    }

    pub(crate) fn external_callable_summary(
        &self,
        metadata: crate::pipeline::ExternalCallableEffectView<'_>,
        span: Span,
    ) -> Option<EffectSummary> {
        let mut summary = EffectSummary::local();
        // Existing external effect metadata does not provide this decomposition.
        summary.independent_determinism = None;
        let handler_requests = match metadata.handler_requests {
            None => crate::RequestInstantiationSummary::Unavailable,
            Some(requests) => crate::RequestInstantiationSummary::Available {
                actions: requests
                    .actions
                    .iter()
                    .map(|instance| {
                        Some(crate::ActionRequestInstance {
                            action: self.external_effect_from_metadata(&instance.action)?,
                            type_bindings: instance.type_bindings.clone(),
                            signature: instance.signature.clone(),
                        })
                    })
                    .collect::<Option<_>>()?,
                parameters: requests
                    .parameters
                    .iter()
                    .map(|parameter| {
                        Some(crate::ParameterRequest {
                            target: parameter_target(
                                metadata.parameter_targets,
                                &parameter.parameter,
                            )?,
                            parameter: parameter.parameter.clone(),
                            span,
                            excluded: parameter
                                .excluded
                                .iter()
                                .map(|effect| self.external_effect_from_metadata(effect))
                                .collect::<Option<_>>()?,
                        })
                    })
                    .collect::<Option<_>>()?,
            },
        };
        let public = self.external_effect_row_from_metadata(metadata.public_effects)?;
        let direct = match metadata.default_handler_effects {
            Some(defaults) => self.external_effect_row_from_metadata(&defaults.direct_effects)?,
            None => public.clone(),
        };
        summary.record_escaping_effect_row(direct.clone());
        self.apply_runtime_support_for_row(&mut summary, &direct);
        let handled_effects = metadata
            .handled_requested_actions
            .effects
            .iter()
            .map(|handled| self.external_effect_from_metadata(handled))
            .collect::<Option<BTreeSet<_>>>()?;
        let defaults = match metadata.default_handler_effects {
            Some(defaults) => {
                Some(self.external_effect_row_from_metadata(&defaults.default_actions)?)
            }
            None => None,
        };
        for action in &metadata.requested_actions.effects {
            let effect = self.external_effect_from_metadata(action)?;
            let handled = handled_effects.contains(&effect);
            let default = defaults
                .as_ref()
                .map_or(handled, |defaults| defaults.effects.contains(&effect));
            if metadata.default_handler_effects.is_none()
                && self.effect_has_default_handler(&effect)
            {
                return None;
            }
            self.record_external_summary_requested_action(
                &mut summary,
                effect.clone(),
                handled,
                default,
                span,
            );
            if handled && !default {
                summary.record_handled_action_row(EffectRow::closed(crate::EffectSet::one(effect)));
                summary.require_runtime(crate::RuntimeRequirementReason::RuntimeHandler);
            }
        }
        if let Some(metadata) = metadata.default_handler_effects {
            if defaults.as_ref().is_some_and(|defaults| {
                defaults.effects.iter().any(|action| {
                    !action.is_action()
                        || !summary.requested_actions.effects.contains(action)
                        || !handled_effects.contains(action)
                })
            }) {
                return None;
            }
            let coverage = crate::EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            let mut seen = BTreeSet::new();
            for handler in &metadata.handlers {
                let action = self.external_effect_from_metadata(&handler.action)?;
                if !self.effect_has_default_handler(&action)
                    || !seen.insert(action.clone())
                    || !summary.default_actions.effects.contains(&action)
                    || !summary.requested_actions.effects.contains(&action)
                {
                    return None;
                }
                let row = self.external_effect_row_from_metadata(&handler.produced)?;
                summary.record_default_handler_effects(action, row).ok()?;
            }
            if summary
                .default_actions
                .effects
                .iter()
                .any(|action| self.effect_has_default_handler(action) && !seen.contains(action))
            {
                return None;
            }
            if summary.escaping_effects.open != public.open
                || !coverage.row_covers(&public, &summary.escaping_effects)
                || !coverage.row_covers(&summary.escaping_effects, &public)
            {
                return None;
            }
        }
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
        summary.default_actions.open = defaults.as_ref().and_then(|row| row.open);
        summary.action_trace = self.external_action_trace_from_metadata(
            metadata.action_trace,
            metadata.parameter_targets,
            span,
        )?;
        summary.handler_requests = handler_requests;
        Some(summary)
    }

    fn external_effect_row_from_metadata(
        &self,
        row: &crate::ExternalEffectRowMetadata,
    ) -> Option<EffectRow> {
        Some(EffectRow {
            effects: row
                .effects
                .iter()
                .map(|effect| self.external_effect_from_metadata(effect))
                .collect::<Option<crate::EffectSet>>()?,
            open: row.tail.as_deref().map(effect_var_id_from_name),
        })
    }

    fn effect_has_default_handler(&self, effect: &Effect) -> bool {
        match effect {
            Effect::Action(action)
            | Effect::AppliedAction(crate::ActionInstanceRef { action, .. }) => self
                .registry
                .action_signature(action)
                .is_some_and(|signature| signature.has_default_handler),
            _ => false,
        }
    }

    fn external_action_trace_from_metadata(
        &self,
        trace: &ExternalActionTraceMetadata,
        parameters: &[(String, crate::ParameterCallTarget)],
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
                    target: parameter_target(parameters, parameter)?,
                    parameter: parameter.clone(),
                    span,
                }
            }
            ExternalActionTraceMetadata::Seq(children) => ActionTraceDomain::Seq(
                children
                    .iter()
                    .map(|child| self.external_action_trace_from_metadata(child, parameters, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ExternalActionTraceMetadata::Choice(children) => ActionTraceDomain::Choice(
                children
                    .iter()
                    .map(|child| self.external_action_trace_from_metadata(child, parameters, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
            ExternalActionTraceMetadata::Repeat(child) => ActionTraceDomain::Repeat(Box::new(
                self.external_action_trace_from_metadata(child, parameters, span)?,
            )),
            ExternalActionTraceMetadata::UnknownOrder(actions) => {
                ActionTraceDomain::UnknownOrder(EffectSet::from_iter(
                    actions
                        .iter()
                        .map(|action| self.external_effect_from_metadata(action))
                        .collect::<Option<Vec<_>>>()?,
                ))
            }
            ExternalActionTraceMetadata::Widened {
                actions,
                parameter_calls,
            } => ActionTraceDomain::Widened {
                actions: EffectSet::from_iter(
                    actions
                        .iter()
                        .map(|action| self.external_effect_from_metadata(action))
                        .collect::<Option<Vec<_>>>()?,
                ),
                parameter_calls: parameter_calls
                    .iter()
                    .map(|parameter| {
                        Some(ParameterCallRef {
                            target: parameter_target(parameters, parameter)?,
                            parameter: parameter.clone(),
                            span,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            },
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
        default: bool,
        span: Span,
    ) {
        if !effect.is_action() {
            self.apply_effect_to_summary(summary, effect, span);
            return;
        }
        match &effect {
            Effect::Action(action) => {
                summary.record_requested_action(effect.clone());
                if default {
                    summary.record_default_handled_action(effect.clone());
                }
                summary.record_action_trace_event(
                    effect.clone(),
                    span,
                    crate::ActionEventSource::ExternalMetadata,
                );
                if (!handled || default)
                    && let Some(reason) = self
                        .registry
                        .runtime_requirement_reason_for_action_ref(action)
                {
                    summary.require_runtime(reason);
                }
            }
            Effect::AppliedAction(action) => {
                summary.record_requested_action(effect.clone());
                if default {
                    summary.record_default_handled_action(effect.clone());
                }
                summary.record_action_trace_event(
                    effect.clone(),
                    span,
                    crate::ActionEventSource::ExternalMetadata,
                );
                if (!handled || default)
                    && let Some(reason) = self
                        .registry
                        .runtime_requirement_reason_for_action_ref(&action.action)
                {
                    summary.require_runtime(reason);
                }
            }
            _ => {}
        }
    }
}
