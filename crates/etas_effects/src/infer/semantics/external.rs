use super::engine::EffectSemantics;
use super::shared::*;
use crate::{external_trace::parameter_target, pipeline::ExternalActionTraceMetadata};

fn request_label(target: &crate::ExternalCallableRequestTarget) -> &str {
    match target {
        crate::ExternalCallableRequestTarget::Parameter(parameter) => parameter,
        crate::ExternalCallableRequestTarget::SpecMethod(method) => &method.method,
    }
}

impl EffectSemantics<'_> {
    fn external_request_target(
        &self,
        target: &crate::ExternalCallableRequestTarget,
        parameters: &[(String, crate::ParameterCallTarget)],
        generics: &[(String, etas_types::CallableGenericParamKind)],
        span: Span,
    ) -> Option<crate::ParameterCallTarget> {
        match target {
            crate::ExternalCallableRequestTarget::Parameter(label) => {
                parameter_target(parameters, label)
            }
            crate::ExternalCallableRequestTarget::SpecMethod(method) => self
                .register_metadata_method(method, parameters, generics, span)
                .ok(),
        }
    }
}

impl EffectSemantics<'_> {
    fn external_metadata_for_callable(
        &self,
        callable: &etas_code::CallableId,
    ) -> Option<&crate::AnchoredExternalMetadata<crate::ExternalEffectSummaryMetadata>> {
        let mut matches = self.external_summaries.iter().filter(|entry| {
            entry
                .callable_contract
                .as_ref()
                .is_some_and(|contract| &contract.header.id == callable)
        });
        let matched = matches.next()?;
        matches.next().is_none().then_some(matched)
    }

    pub(super) fn external_reference_metadata(
        &self,
        value: HirExprId,
    ) -> Option<&crate::AnchoredExternalMetadata<crate::ExternalEffectSummaryMetadata>> {
        let HirExpr::Path(path) = self.hir.exprs.get(value)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.type_symbols.canonical_symbol(self.hir, symbol)?;
        let definition = self.types.facts.external_definition_bindings.get(&symbol);
        let standard;
        let definition = match definition {
            Some(definition) => definition,
            None => {
                let etas_hir::SymbolDef::ImportAlias { path, .. } =
                    &self.hir.symbols.get(symbol)?.def
                else {
                    return None;
                };
                standard = self
                    .std_registry
                    .lookup_qualified(path)
                    .and_then(|symbol| self.std_registry.body_binding(symbol.id))?
                    .callable
                    .as_declaration()?
                    .clone();
                &standard
            }
        };
        if definition.kind() != etas_code::DefinitionKind::Flow {
            return None;
        }
        let callable = etas_code::CallableId::declaration(definition.clone()).ok()?;
        self.external_metadata_for_callable(&callable)
    }

    pub(super) fn external_reflection_metadata(
        &self,
        reflection: HirExprId,
    ) -> Option<&crate::AnchoredExternalMetadata<crate::ExternalEffectSummaryMetadata>> {
        let fact = self.types.facts.reflections.get(&reflection)?;
        let etas_types::facts::reflection::CheckedReflectionTarget::ExternalCallable {
            ref callable,
            ..
        } = fact.target
        else {
            return None;
        };
        self.external_metadata_for_callable(callable)
    }

    pub(super) fn external_reference_summary(
        &self,
        value: HirExprId,
        span: Span,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        use super::specialize::EffectSpecializationError as Error;
        let metadata = self
            .external_reference_metadata(value)
            .ok_or(Error::InvalidCallableReturnSources)?;
        let signature = self
            .checked_callable_type_for_expr(value)
            .ok_or(Error::InvalidCallableParameterInterface)?;
        self.external_instantiated_summary(metadata, signature, value, span)
    }

    fn external_instantiated_summary(
        &self,
        metadata: &crate::ExternalEffectSummaryMetadata,
        signature: &etas_types::FlowType,
        instantiation: HirExprId,
        span: Span,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        use super::specialize::EffectSpecializationError as Error;
        let contract = metadata
            .callable_contract
            .as_ref()
            .ok_or(Error::InvalidCallableReturnSources)?;
        let checked = self.call_generic_bindings(instantiation);
        if !checked.deferred_effect_rows.is_empty()
            || metadata
                .generic_params
                .iter()
                .any(|(name, kind)| match kind {
                    etas_types::CallableGenericParamKind::Type => {
                        !checked.types.iter().any(|(bound, _)| bound == name)
                    }
                    etas_types::CallableGenericParamKind::Effect => {
                        !checked.effect_rows.iter().any(|(bound, _)| bound == name)
                    }
                })
        {
            return Err(Error::InvalidCallableParameterInterface);
        }
        let rows = checked
            .effect_rows
            .iter()
            .map(|(name, row)| (name.clone(), self.row_from_type_ref(row)))
            .collect::<Vec<_>>();
        let mut summary = self
            .external_summary_from_metadata(metadata, span)
            .ok_or(Error::InvalidCallableReturnSources)?;
        self.separate_symbolic_parameter_requests(metadata, instantiation, &mut summary)?;
        summary.exits = self.external_callback_outcomes(
            contract,
            &summary,
            signature,
            span,
            &checked.types,
            &metadata.generic_params,
        )?;
        self.specialize_summary_with_bindings(&summary, &[], &checked.types, &rows)
    }

    fn separate_symbolic_parameter_requests(
        &self,
        metadata: &crate::ExternalEffectSummaryMetadata,
        expression: HirExprId,
        summary: &mut EffectSummary,
    ) -> Result<(), super::specialize::EffectSpecializationError> {
        use super::specialize::EffectSpecializationError as Error;
        let Some(open) = summary.requested_actions.open else {
            return Ok(());
        };
        let path = match self.hir.exprs.get(expression) {
            Some(HirExpr::Reflect(reflection)) => reflection.target.root_path(),
            Some(HirExpr::Path(path)) => path,
            _ => return Err(Error::InvalidCallableParameterInterface),
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(Error::InvalidCallableParameterInterface);
        };
        let Some(etas_types::SymbolTypeFact::Flow { signature }) =
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol)
        else {
            return Err(Error::InvalidCallableParameterInterface);
        };
        if signature.params.len() != metadata.param_names.len() {
            return Err(Error::InvalidCallableParameterInterface);
        }
        let Some(requests) = metadata.handler_requests.as_ref() else {
            return Err(Error::InvalidCallableReturnSources);
        };
        let mut proven = false;
        for request in &requests.parameters {
            let crate::ExternalCallableRequestTarget::Parameter(parameter) = &request.parameter
            else {
                continue;
            };
            let mut positions = metadata
                .param_names
                .iter()
                .enumerate()
                .filter_map(|(position, name)| (name == parameter).then_some(position));
            let Some(position) = positions.next() else {
                return Err(Error::InvalidCallableParameterInterface);
            };
            if positions.next().is_some() {
                return Err(Error::InvalidCallableParameterInterface);
            }
            let position_u32 =
                u32::try_from(position).map_err(|_| Error::InvalidCallableParameterInterface)?;
            let Some(etas_types::Type::Function(function)) =
                self.types.store.get(signature.params[position])
            else {
                return Err(Error::InvalidCallableParameterInterface);
            };
            let Some(name) = function
                .effects
                .as_ref()
                .and_then(|row| row.tail.as_deref())
            else {
                continue;
            };
            if effect_var_id_from_name(name) == open
                && signature.generic_params.iter().any(|parameter| {
                    parameter.name == name
                        && parameter.kind == etas_types::CallableGenericParamKind::Effect
                })
                && trace_calls_interface_parameter(&summary.action_trace, position_u32)
            {
                proven = true;
            }
        }
        if !proven {
            return Err(Error::InvalidCallableEnvironment {
                reason:
                    "open external requested-action row has no checked parameter-call provenance"
                        .into(),
            });
        }
        summary.requested_actions.open = None;
        Ok(())
    }

    pub(super) fn external_frozen_summary(
        &self,
        reflection: HirExprId,
        span: Span,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        use super::specialize::EffectSpecializationError as Error;
        let metadata = self
            .external_reflection_metadata(reflection)
            .ok_or(Error::InvalidCallableReturnSources)?;
        let fact = self
            .types
            .facts
            .reflections
            .get(&reflection)
            .ok_or(Error::InvalidCallableReturnSources)?;
        let ty = self
            .descriptor_contract_type(fact.descriptor_type)
            .ok_or(Error::InvalidCallableParameterInterface)?;
        let Some(etas_types::Type::Function(signature)) = self.types.store.get(ty) else {
            return Err(Error::InvalidCallableParameterInterface);
        };
        self.external_instantiated_summary(metadata, signature, reflection, span)
    }

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
        self.external_call_summary_from_metadata(metadata, site, state, None)
    }

    pub(super) fn external_call_summary_from_metadata(
        &self,
        metadata: &crate::ExternalEffectSummaryMetadata,
        site: &CallSite<EffectUnit>,
        state: &mut EffectState,
        generic_source: Option<HirExprId>,
    ) -> Result<Option<EffectSummary>, super::specialize::EffectSpecializationError> {
        let Some(mut summary) = self.external_summary_from_metadata(metadata, site.span) else {
            return Ok(None);
        };
        let bindings = self.call_bindings_from_param_names(site, &metadata.param_names);
        let parameter_targets = (0..metadata.param_names.len())
            .map(|position| crate::ParameterCallTarget::Interface(position as u32))
            .collect::<Vec<_>>();
        let mut trace_bindings = self.call_trace_bindings(site, &parameter_targets)?;
        let signature = self
            .checked_callable_type_for_expr(site.callee_expr)
            .ok_or(
                super::specialize::EffectSpecializationError::InvalidCallableParameterInterface,
            )?;
        let (type_bindings, effect_row_bindings) =
            self.checked_call_generic_values(generic_source.unwrap_or(site.call), state)?;
        if let Some(contract) = &metadata.callable_contract {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(super::specialize::EffectSpecializationError::Interrupted)?;
            }
            summary.exits = self.external_callback_outcomes(
                contract,
                &summary,
                signature,
                site.span,
                &type_bindings,
                &metadata.generic_params,
            )?;
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(super::specialize::EffectSpecializationError::Interrupted)?;
            }
        }
        self.bind_spec_method_arguments(
            &summary,
            &mut trace_bindings,
            self.callable_environment_site(site.call)?,
            state,
            super::callable_generics::ScopedGenericArguments::named(
                &type_bindings,
                &effect_row_bindings,
            ),
        )?;
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
        if let Some(exits) = &mut summary.exits {
            for exit in &mut exits.exits {
                exit.trace =
                    self.specialize_parameter_call_trace(&exit.trace, &trace_bindings, state)?;
            }
        }
        self.solve_callback_completion(&mut summary, &trace_bindings, &type_bindings, state)?;
        self.forward_callback_outcomes(&mut summary, &trace_bindings, &parameter_targets, state)?;
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
                generic_params: &metadata.generic_params,
                handler_requests: metadata.handler_requests.as_ref(),
                parameter_targets: &parameter_targets,
                public_effects: &metadata.public_effects,
                requested_actions: &metadata.requested_actions,
                handled_requested_actions: &metadata.handled_requested_actions,
                default_handler_effects: metadata.default_handler_effects.as_ref(),
                action_trace: &metadata.action_trace,
                symbolic_parameter_bounds: &[],
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
                            target: self.external_request_target(
                                &parameter.parameter,
                                metadata.parameter_targets,
                                metadata.generic_params,
                                span,
                            )?,
                            parameter: request_label(&parameter.parameter).to_owned(),
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
        let requested = metadata
            .requested_actions
            .effects
            .iter()
            .map(|effect| self.external_effect_from_metadata(effect))
            .collect::<Option<crate::EffectSet>>()?;
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let independent = crate::EffectRow::closed(crate::EffectSet::from_iter(
            direct
                .effects
                .iter()
                .filter(|effect| {
                    requested.contains(effect)
                        || !metadata.symbolic_parameter_bounds.iter().any(|bound| {
                            bound.open.is_none()
                                && coverage.row_covers(
                                    bound,
                                    &crate::EffectRow::closed(crate::EffectSet::one(
                                        (*effect).clone(),
                                    )),
                                )
                        })
                })
                .cloned(),
        ));
        self.apply_runtime_support_for_row(&mut summary, &independent);
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
            metadata.generic_params,
            span,
        )?;
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        if !coverage.row_covers(
            &summary.requested_actions,
            &crate::EffectRow::closed(summary.action_trace.action_set()),
        ) {
            return None;
        }
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
        generics: &[(String, etas_types::CallableGenericParamKind)],
        span: Span,
    ) -> Option<ActionTraceDomain> {
        crate::external_trace::lower_external_action_trace_with_targets(
            trace,
            span,
            |action| self.external_effect_from_metadata(action),
            |parameter| self.external_request_target(parameter, parameters, generics, span),
        )
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
        if self.registry.action_by_name(&name).is_some() {
            return crate::external_trace::lower_external_action(
                self.registry,
                &self.types.store,
                metadata,
            );
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

fn trace_calls_interface_parameter(trace: &crate::ActionTraceDomain, position: u32) -> bool {
    use crate::{ActionTraceDomain, ParameterCallTarget};
    let target = ParameterCallTarget::Interface(position);
    let mut pending = vec![trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::ParameterCall { target: actual, .. } if *actual == target => {
                return true;
            }
            ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                pending.extend(parts)
            }
            ActionTraceDomain::Repeat(inner) => pending.push(inner),
            ActionTraceDomain::Widened {
                parameter_calls, ..
            } if parameter_calls.iter().any(|call| call.target == target) => return true,
            _ => {}
        }
    }
    false
}
