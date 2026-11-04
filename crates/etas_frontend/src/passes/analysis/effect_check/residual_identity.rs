use std::cell::OnceCell;

use etas_code::{
    CodeFingerprint,
    descriptor::{DescriptorLimits, GenericOwner},
};
use etas_hir::HirItemId;
use etas_types::TypeId;

use crate::code::{
    descriptor::{DefinitionDescriptorProjector, DescriptorInput, ProjectionError},
    identity::{DefinitionIdentityIndex, DefinitionOrigins, standard_package_key},
};
use etas_effects::ResidualIdentityError;

fn projection_error(error: ProjectionError) -> ResidualIdentityError {
    match error.resource_limit() {
        Some((resource, limit)) => ResidualIdentityError::ResourceLimit { resource, limit },
        None => ResidualIdentityError::Unavailable(error.to_string()),
    }
}

fn descriptor_error(error: etas_code::descriptor::DescriptorError) -> ResidualIdentityError {
    match error {
        etas_code::descriptor::DescriptorError::Limit { resource, limit } => {
            ResidualIdentityError::ResourceLimit { resource, limit }
        }
        error => ResidualIdentityError::Unavailable(error.to_string()),
    }
}

#[derive(Clone, Copy)]
pub(in crate::passes::analysis) struct CheckedResidualIdentityInput<'a> {
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeOutput,
    pub std_registry: &'a etas_std::StdRegistry,
    pub imports: &'a crate::ResolvedImports,
    pub modules: &'a crate::ModuleIndex,
    pub sources: &'a crate::SourceSet,
    pub parsed: &'a [crate::ParsedSource],
    pub environment: &'a crate::ValidatedExternalEnvironment,
    pub overlay: Option<&'a crate::code::draft::DefaultBodyOverlay>,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
    pub foreign_type_bindings: Option<&'a crate::code::definition::ForeignTypeBindings>,
    pub limits: DescriptorLimits,
}

/// Session-local projection from checked facts, not installation evidence.
pub(in crate::passes::analysis) struct CheckedResidualIdentities<'a> {
    hir: &'a etas_hir::HirRevision,
    types: &'a etas_types::TypeOutput,
    std_registry: &'a etas_std::StdRegistry,
    imports: &'a crate::ResolvedImports,
    modules: &'a crate::ModuleIndex,
    sources: &'a crate::SourceSet,
    parsed: &'a [crate::ParsedSource],
    environment: &'a crate::ValidatedExternalEnvironment,
    overlay: Option<&'a crate::code::draft::DefaultBodyOverlay>,
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
    foreign_type_bindings: Option<&'a crate::code::definition::ForeignTypeBindings>,
    limits: DescriptorLimits,
    source_bundle: OnceCell<crate::SourceBundle>,
    // The address only prevents reuse with another session-local registry; it
    // is never included in portable identities or serialized artifacts.
    index: OnceCell<(
        usize,
        Result<DefinitionIdentityIndex, etas_effects::ResidualIdentityError>,
    )>,
}

impl<'a> CheckedResidualIdentities<'a> {
    pub fn new(input: CheckedResidualIdentityInput<'a>) -> Self {
        let CheckedResidualIdentityInput {
            hir,
            types,
            std_registry,
            imports,
            modules,
            sources,
            parsed,
            environment,
            overlay,
            control,
            foreign_type_bindings,
            limits,
        } = input;
        Self {
            hir,
            types,
            std_registry,
            imports,
            modules,
            sources,
            parsed,
            environment,
            overlay,
            control,
            foreign_type_bindings,
            limits,
            source_bundle: OnceCell::new(),
            index: OnceCell::new(),
        }
    }

    fn input<'b>(&'b self, registry: &'b etas_effects::EffectRegistry) -> DescriptorInput<'b> {
        DescriptorInput {
            hir: self.hir,
            sources: None,
            types: &self.types.facts,
            type_store: &self.types.store,
            std_registry: self.std_registry,
            resolved_imports: self.imports,
            module_index: self.modules,
            effect_registry: registry,
            effects: None,
            foreign_type_bindings: self.foreign_type_bindings,
        }
    }

    pub(in crate::passes::analysis) fn index(
        &self,
        registry: &etas_effects::EffectRegistry,
    ) -> Result<&DefinitionIdentityIndex, ResidualIdentityError> {
        self.checkpoint()?;
        let address = std::ptr::from_ref(registry) as usize;
        let (cached_address, result) = self.index.get_or_init(|| {
            let result = (|| {
                let origins = if let Some(overlay) = self.overlay {
                    overlay
                        .frozen()
                        .origins_for_hir(self.hir)
                        .map_err(projection_error)?
                } else {
                    DefinitionOrigins {
                        source_modules: DefinitionOrigins::source_modules_from_inputs(
                            self.hir,
                            self.modules,
                            self.sources,
                            self.parsed,
                            self.environment,
                        )
                        .map_err(projection_error)?,
                        standard_package: standard_package_key(self.std_registry)
                            .map_err(projection_error)?,
                        external_definitions:
                            DefinitionOrigins::external_definitions_from_environment(
                                self.environment,
                            )
                            .map_err(projection_error)?,
                    }
                };
                DefinitionIdentityIndex::from_input_with_limits(
                    self.input(registry),
                    &origins,
                    self.limits,
                )
                .map_err(projection_error)
            })();
            (address, result)
        });
        if *cached_address != address {
            return Err(ResidualIdentityError::Unavailable(
                "residual identity projection used a different effect registry".into(),
            ));
        }
        self.checkpoint()?;
        result.as_ref().map_err(Clone::clone)
    }

    fn owner(
        &self,
        index: &DefinitionIdentityIndex,
        item: HirItemId,
    ) -> Result<GenericOwner, ResidualIdentityError> {
        let symbol = self
            .hir
            .items
            .get(item)
            .and_then(etas_hir::HirItem::symbol)
            .ok_or_else(|| {
                ResidualIdentityError::Unavailable(
                    "checked generic callable has no declaration symbol".into(),
                )
            })?;
        if let Some(method) = index.method(symbol) {
            Ok(GenericOwner::Method(method.clone()))
        } else {
            index
                .symbol(symbol)
                .cloned()
                .map(GenericOwner::Declaration)
                .ok_or_else(|| {
                    ResidualIdentityError::Unavailable(
                        "checked generic callable has no canonical identity".into(),
                    )
                })
        }
    }

    /// The identity index used for this run's canonical projections, so that
    /// later consumers project concrete actions with the same identities.
    pub fn into_identity_index(self) -> Option<DefinitionIdentityIndex> {
        self.index.into_inner().and_then(|(_, index)| index.ok())
    }

    fn checkpoint(&self) -> Result<(), ResidualIdentityError> {
        if let Some(control) = self.control {
            control
                .checkpoint()
                .map_err(ResidualIdentityError::Interrupted)?;
        }
        Ok(())
    }
}

impl etas_core::memory::RetainedSize for CheckedResidualIdentities<'_> {
    fn visit_retained<'a>(&'a self, visitor: &mut etas_core::memory::MemoryVisitor<'a>) {
        if let Some((_, index)) = self.index.get() {
            etas_core::memory::RetainedSize::visit_retained(index, visitor);
        }
    }
}

impl etas_effects::ResidualIdentityProvider for CheckedResidualIdentities<'_> {
    fn closed_type(
        &self,
        registry: &etas_effects::EffectRegistry,
        owner: HirItemId,
        ty: TypeId,
    ) -> Result<CodeFingerprint, ResidualIdentityError> {
        let index = self.index(registry)?;
        let mut projector =
            DefinitionDescriptorProjector::from_input(self.input(registry), index, self.limits);
        let references = projector
            .project_closed_types(self.owner(index, owner)?, &[ty])
            .map_err(projection_error)?;
        let reference = *references.first().ok_or_else(|| {
            ResidualIdentityError::Unavailable("checked type projection is empty".into())
        })?;
        let fingerprint = projector
            .finish()
            .map_err(projection_error)?
            .type_fingerprint(reference)
            .map_err(descriptor_error)?;
        self.checkpoint()?;
        Ok(fingerprint)
    }

    fn closed_row(
        &self,
        registry: &etas_effects::EffectRegistry,
        owner: HirItemId,
        row: &etas_effects::EffectRow,
    ) -> Result<CodeFingerprint, ResidualIdentityError> {
        let index = self.index(registry)?;
        let mut projector =
            DefinitionDescriptorProjector::from_input(self.input(registry), index, self.limits);
        let reference = projector
            .project_closed_analysis_row(self.owner(index, owner)?, row)
            .map_err(projection_error)?;
        let fingerprint = projector
            .finish()
            .map_err(projection_error)?
            .row_fingerprint(reference)
            .map_err(descriptor_error)?;
        self.checkpoint()?;
        Ok(fingerprint)
    }
}

impl etas_effects::TraceMonitorProjector for CheckedResidualIdentities<'_> {
    /// Projects exactly the analyzed model of one conformance through the
    /// checked canonical descriptor projector, including source-link and
    /// spec-snapshot validation. The result is only a lowering input.
    fn project_trace_monitor(
        &self,
        request: etas_effects::TraceMonitorProjectionRequest<'_>,
    ) -> Result<etas_code::residual::monitor::TraceComponentMonitor, ResidualIdentityError> {
        use crate::code::descriptor::{TraceSourceLinkLimits, TraceTemplateProjectionError};
        let registry = request.registry;
        let index = self.index(registry)?;
        let item = request.lowering.item;
        let owner = self.owner(index, item)?;
        let sources = self
            .source_bundle
            .get_or_init(|| self.sources.to_source_bundle());
        let input = DescriptorInput {
            sources: Some(sources),
            effects: Some(request.facts),
            ..self.input(registry)
        };
        let signature =
            crate::code::definition::callable_signature(input, item).ok_or_else(|| {
                ResidualIdentityError::Unavailable(
                    "checked trace owner has no callable signature".into(),
                )
            })?;
        let (names, declared) = crate::code::definition::callable_source_header(input, item)
            .map_err(projection_error)?;
        let mut projector = DefinitionDescriptorProjector::from_input(input, index, self.limits);
        projector
            .project_signature(owner.clone(), signature, &names, declared)
            .map_err(projection_error)?;
        let limits = request.source_limits;
        let source_limits = TraceSourceLinkLimits {
            max_sources: limits.max_sources,
            max_sites: limits.max_sites,
            max_clauses: limits.max_clauses,
            max_snapshot_bytes: limits.max_snapshot_bytes,
            identity: limits.identity,
        };
        let projection = if names.is_empty() {
            projector
                .finish_analyzed_trace_template(&owner, request.lowering, source_limits)
                .map(|projection| {
                    etas_code::residual::monitor::TraceComponentMonitor::Concrete(
                        std::sync::Arc::new(projection.into_monitor()),
                    )
                })
        } else {
            projector
                .finish_analyzed_symbolic_trace_template(
                    &owner,
                    request.lowering,
                    source_limits,
                    etas_code::residual::pattern::PatternTemplateLimits {
                        pattern: request.lowering.limits.evidence.pattern,
                        types: Default::default(),
                        descriptor: self.limits,
                        max_binding_steps:
                            etas_code::residual::type_template::TypeTemplateLimits::default()
                                .max_work,
                    },
                )
                .map(|projection| {
                    etas_code::residual::monitor::TraceComponentMonitor::Symbolic(
                        projection.template,
                    )
                })
        }
        .map_err(|error| match error {
            TraceTemplateProjectionError::Projection(error) => projection_error(error),
            TraceTemplateProjectionError::Lower(
                etas_effects::trace_spec::TraceRulesLoweringError::Interrupted(interruption),
            ) => ResidualIdentityError::Interrupted(interruption),
            error => ResidualIdentityError::Unavailable(format!(
                "trace monitor projection is unsupported: {error:?}"
            )),
        })?;
        self.checkpoint()?;
        Ok(projection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_effects::ResidualIdentityProvider;
    use etas_types::ItemSignature;

    #[test]
    fn checked_projection_preserves_nominal_and_action_row_identity() {
        let source = r#"
module app;
type Token = i32;
effect Gate { action tick() -> unit; }
flow generic<T>(value: T) -> unit ![] { return; }
flow invoke<effect E>(f: () -> unit ![E]) -> unit ![E] { f(); return; }
flow both<effect E>(f: () -> unit ![E], g: () -> unit ![E]) -> unit ![E] { f(); g(); return; }
flow forward<effect X>(f: () -> unit ![X]) -> unit ![X] { invoke<![X]>(f); return; }
flow token(value: Token) -> Token ![] { return value; }
flow action() -> unit ![Gate.tick] { perform Gate.tick(); return; }
flow use_token(value: Token) -> unit ![] { generic<Token>(value); return; }
flow use_action() -> unit ![Gate.tick] { invoke<![Gate.tick]>(action); return; }
flow use_forward() -> unit ![Gate.tick] { forward<![Gate.tick]>(action); return; }
flow use_inferred() -> unit ![Gate.tick] { invoke(action); return; }
flow use_lambda() -> unit ![Gate.tick] { invoke(() => { perform Gate.tick(); return; }); return; }
flow use_both() -> unit ![Gate.tick] { both(() => { perform Gate.tick(); return; }, () => { perform Gate.tick(); return; }); return; }
"#;
        let mut input = crate::ProjectInput::single_source(crate::SourceInput::anonymous(source));
        input.options.entry_policy = crate::EntryPolicy::Optional;
        let package = etas_code::PackageKey::from_name_and_version("app", "1.0.0").unwrap();
        input.environment.local_source_package = Some(package);
        let source_key = etas_code::StableSourceKey::new(
            etas_code::StableSourceOrigin::Package(package),
            &["app.es".into()],
        )
        .unwrap();
        input
            .environment
            .stable_source_keys
            .push(crate::ProjectStableSourceKeyInput {
                source: input.sources[0].id,
                key: source_key,
            });
        let output = crate::Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let types = etas_types::TypeOutput {
            facts: checked.types.clone(),
            store: checked.type_store.clone(),
            diagnostics: vec![],
        };
        let initial_types = etas_types::check_program(&checked.hir);
        assert!(
            initial_types.diagnostics.is_empty(),
            "{:?}",
            initial_types.diagnostics
        );
        let provider_input = CheckedResidualIdentityInput {
            hir: &checked.hir,
            types: &types,
            std_registry: &checked.std_registry,
            imports: &checked.resolved_imports,
            modules: &checked.module_index,
            sources: checked.definition_sources.source_set(),
            parsed: checked.definition_sources.parsed_sources(),
            environment: &checked.definition_environment,
            overlay: None,
            control: None,
            foreign_type_bindings: Some(&checked.foreign_type_bindings),
            limits: DescriptorLimits::default(),
        };
        let provider = CheckedResidualIdentities::new(provider_input);
        let find = |name: &str| {
            checked
                .hir
                .items
                .iter()
                .find_map(|(id, item)| match item {
                    etas_hir::HirItem::Flow(flow)
                        if checked.hir.symbols.get(flow.symbol).unwrap().name == name =>
                    {
                        Some(id)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let generic = find("generic");
        let invoke = find("invoke");
        let both = find("both");
        let forward = find("forward");
        let token = find("token");
        let action = find("action");
        let use_token = find("use_token");
        let use_action = find("use_action");
        let use_forward = find("use_forward");
        let use_inferred = find("use_inferred");
        let use_lambda = find("use_lambda");
        let use_both = find("use_both");
        assert!(
            initial_types
                .facts
                .generic_instantiations
                .iter()
                .any(|(call, fact)| {
                    checked.hir.index().expr_item.get(call) == Some(&use_lambda)
                        && !fact.deferred_effect_row_obligations.is_empty()
                }),
            "pre-effect lambda must retain deferred effect-row obligations: {:?}",
            initial_types.facts.generic_instantiations
        );
        assert!(
            initial_types
                .facts
                .generic_instantiations
                .iter()
                .any(|(call, fact)| {
                    checked.hir.index().expr_item.get(call) == Some(&use_both)
                        && fact.deferred_effect_row_obligations.len() == 2
                }),
            "both callbacks must have independent deferred obligations"
        );
        assert!(
            types
                .facts
                .generic_instantiations
                .iter()
                .any(|(call, fact)| {
                    checked.hir.index().expr_item.get(call) == Some(&use_lambda)
                        && fact.effect_row_bindings.iter().any(|(name, row)| {
                            name == "E" && !row.effects.is_empty() && row.tail.is_none()
                        })
                }),
            "lambda must have a solved, non-empty effect row"
        );
        let ItemSignature::Flow(token_signature) = &types.facts.item_signatures[&token] else {
            panic!("flow signature")
        };
        let ItemSignature::Flow(action_signature) = &types.facts.item_signatures[&action] else {
            panic!("flow signature")
        };
        let ty = token_signature.params[0];
        let row = &checked.effects.item_effects[&action].requested_actions;
        assert!(!row.effects.is_empty());
        let index = provider.index(&checked.effect_registry).unwrap();
        let owner = provider.owner(index, generic).unwrap();
        let mut expected =
            DefinitionDescriptorProjector::new(&checked, index, DescriptorLimits::default());
        let type_ref = expected.project_closed_types(owner.clone(), &[ty]).unwrap()[0];
        let row_ref = expected
            .project_closed_rows(owner, &[action_signature.effects.clone().unwrap()])
            .unwrap()[0];
        let graph = expected.finish().unwrap();
        assert_eq!(
            provider
                .closed_type(&checked.effect_registry, generic, ty)
                .unwrap(),
            graph.type_fingerprint(type_ref).unwrap()
        );
        assert_eq!(
            provider
                .closed_row(&checked.effect_registry, generic, row)
                .unwrap(),
            graph.row_fingerprint(row_ref).unwrap()
        );
        let other_registry = (*checked.effect_registry).clone();
        assert!(provider.closed_type(&other_registry, generic, ty).is_err());
        struct Cancel;
        impl etas_utils::PipelineExecutionControl for Cancel {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancelled".into(),
                })
            }
        }
        let cancelled = CheckedResidualIdentities::new(CheckedResidualIdentityInput {
            control: Some(&Cancel),
            ..provider_input
        });
        assert!(matches!(
            cancelled.closed_type(&checked.effect_registry, generic, ty),
            Err(ResidualIdentityError::Interrupted(_))
        ));
        let limited = CheckedResidualIdentities::new(CheckedResidualIdentityInput {
            limits: DescriptorLimits {
                max_nodes: 0,
                ..DescriptorLimits::default()
            },
            ..provider_input
        });
        assert!(matches!(
            limited.closed_type(&checked.effect_registry, generic, ty),
            Err(ResidualIdentityError::ResourceLimit { .. })
        ));

        #[derive(etas_core::RetainedSize)]
        struct Frozen {
            callables: std::collections::HashMap<HirItemId, etas_effects::FrozenCallableSummary>,
        }
        impl etas_effects::FrozenCallableSummaryProvider for Frozen {
            fn project(
                &self,
                _: &etas_effects::EffectRegistry,
            ) -> Result<etas_effects::FrozenCallableEnvironment, etas_effects::EffectPipelineError>
            {
                Ok(etas_effects::FrozenCallableEnvironment {
                    callables: self.callables.clone(),
                    ..Default::default()
                })
            }
        }
        use etas_code::descriptor::GenericParameterKind;
        use etas_code::residual::identity::{
            CheckedSourceSnapshot, GenericArgumentIdentity, GenericParameterId, ObligationId,
            ResidualObligationRef, ResidualOwnerId, ResidualOwnerKind,
        };
        let source_file = &checked.definition_sources.source_set().files[0];
        let snapshot = CheckedSourceSnapshot::new(source_key, source_file.text.as_bytes());
        let parameter = |item: HirItemId, kind| {
            let span = checked.hir.items.get(item).unwrap().span();
            let location = snapshot
                .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
                .unwrap();
            let owner = ResidualOwnerId::from_checked_source_declaration(
                &snapshot,
                &location,
                ResidualOwnerKind::Flow,
            )
            .unwrap();
            GenericParameterId::from_source_owner(owner, 0, kind)
        };
        let type_parameter = parameter(generic, GenericParameterKind::Type);
        let row_parameter = parameter(invoke, GenericParameterKind::EffectRow);
        let both_parameter = parameter(both, GenericParameterKind::EffectRow);
        let forward_parameter = parameter(forward, GenericParameterKind::EffectRow);
        let obligation =
            ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([0x77; 32]));
        let summary = |item, parameter, argument| {
            let mut summary = checked.effects.item_effects[&item].clone();
            summary.residual_checks.insert(
                ResidualObligationRef::new(
                    obligation,
                    [(parameter, argument)].into_iter().collect(),
                )
                .unwrap(),
            );
            etas_effects::FrozenCallableSummary::Solved(std::sync::Arc::new(summary))
        };
        let frozen = Frozen {
            callables: [
                (
                    generic,
                    summary(
                        generic,
                        type_parameter,
                        GenericArgumentIdentity::TypeParameter(type_parameter),
                    ),
                ),
                (
                    invoke,
                    summary(
                        invoke,
                        row_parameter,
                        GenericArgumentIdentity::EffectRowParameter(row_parameter),
                    ),
                ),
                (
                    both,
                    summary(
                        both,
                        both_parameter,
                        GenericArgumentIdentity::EffectRowParameter(both_parameter),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let source_snapshots = [etas_effects::EffectSourceSnapshot {
            source: source_file.id,
            key: source_key,
            text: &source_file.text,
        }];
        let pipeline_provider = CheckedResidualIdentities::new(CheckedResidualIdentityInput {
            hir: &checked.hir,
            types: &initial_types,
            std_registry: &checked.std_registry,
            imports: &checked.resolved_imports,
            modules: &checked.module_index,
            sources: checked.definition_sources.source_set(),
            parsed: checked.definition_sources.parsed_sources(),
            environment: &checked.definition_environment,
            overlay: None,
            control: None,
            foreign_type_bindings: Some(&checked.foreign_type_bindings),
            limits: DescriptorLimits::default(),
        });
        let result = etas_effects::RunEffectPipeline::run(etas_effects::EffectPipelineInput {
            external_implementations: &[],
            source_snapshots: &source_snapshots,
            residual_identity: Some(&pipeline_provider),
            trace_monitor_projector: None,
            frozen_calls: Some(&frozen),
            hir: &checked.hir,
            types: &initial_types,
            std_registry: &checked.std_registry,
            dependency_metadata: None,
            tool_bindings: &[],
            external_summaries: &[],
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        })
        .unwrap();
        let token_checks = &result.effects.facts.item_effects[&use_token].residual_checks;
        let action_checks = &result.effects.facts.item_effects[&use_action].residual_checks;
        let forward_checks = &result.effects.facts.item_effects[&forward].residual_checks;
        let use_forward_checks = &result.effects.facts.item_effects[&use_forward].residual_checks;
        let inferred_checks = &result.effects.facts.item_effects[&use_inferred].residual_checks;
        let lambda_checks = &result.effects.facts.item_effects[&use_lambda].residual_checks;
        let both_checks = &result.effects.facts.item_effects[&use_both].residual_checks;
        assert_eq!(
            result.effects.facts.item_effects[&use_lambda].requested_actions,
            checked.effects.item_effects[&use_lambda].requested_actions,
        );
        assert_eq!(
            result.effects.facts.item_effects[&use_lambda].escaping_effects,
            checked.effects.item_effects[&use_lambda].escaping_effects
        );
        assert_eq!(
            result.effects.facts.item_effects[&use_lambda].default_actions,
            checked.effects.item_effects[&use_lambda].default_actions
        );
        assert_eq!(
            result.effects.facts.item_effects[&use_both].requested_actions,
            checked.effects.item_effects[&use_both].requested_actions
        );
        assert_eq!(
            result.effects.facts.item_effects[&use_both].action_trace,
            checked.effects.item_effects[&use_both].action_trace
        );
        let action = row.effects.iter().next().unwrap();
        let mut work = vec![&result.effects.facts.item_effects[&use_both].action_trace];
        let mut occurrences = 0;
        while let Some(trace) = work.pop() {
            match trace {
                etas_effects::ActionTraceDomain::Empty => {}
                etas_effects::ActionTraceDomain::Event(event) if &event.action == action => {
                    occurrences += 1
                }
                etas_effects::ActionTraceDomain::Seq(parts) => work.extend(parts),
                other => panic!("unexpected callback trace: {other:?}"),
            }
        }
        assert_eq!(occurrences, 2);
        assert_eq!(
            result.effects.facts.item_effects[&use_lambda].action_trace,
            checked.effects.item_effects[&use_lambda].action_trace,
        );
        assert!(token_checks.iter().any(
            |reference| reference.substitutions().get(&type_parameter)
                == Some(&GenericArgumentIdentity::Type(
                    graph.type_fingerprint(type_ref).unwrap()
                ))
        ));
        assert!(action_checks.iter().any(
            |reference| reference.substitutions().get(&row_parameter)
                == Some(&GenericArgumentIdentity::EffectRow(
                    graph.row_fingerprint(row_ref).unwrap()
                ))
        ));
        assert!(
            forward_checks
                .iter()
                .any(|reference| reference.obligation() == obligation
                    && reference.substitutions().get(&row_parameter)
                        == Some(&GenericArgumentIdentity::EffectRowParameter(
                            forward_parameter
                        )))
        );
        assert!(
            use_forward_checks
                .iter()
                .any(|reference| reference.obligation() == obligation
                    && reference.substitutions().get(&row_parameter)
                        == Some(&GenericArgumentIdentity::EffectRow(
                            graph.row_fingerprint(row_ref).unwrap()
                        )))
        );
        assert!(
            inferred_checks
                .iter()
                .any(|reference| reference.obligation() == obligation
                    && reference.substitutions().get(&row_parameter)
                        == Some(&GenericArgumentIdentity::EffectRow(
                            graph.row_fingerprint(row_ref).unwrap()
                        )))
        );
        assert!(
            lambda_checks
                .iter()
                .any(|reference| reference.obligation() == obligation
                    && reference.substitutions().get(&row_parameter)
                        == Some(&GenericArgumentIdentity::EffectRow(
                            graph.row_fingerprint(row_ref).unwrap()
                        )))
        );
        assert!(
            both_checks
                .iter()
                .any(|reference| reference.obligation() == obligation
                    && reference.substitutions().get(&both_parameter)
                        == Some(&GenericArgumentIdentity::EffectRow(
                            graph.row_fingerprint(row_ref).unwrap()
                        )))
        );
        assert!(
            !result.effects.diagnostics.is_empty(),
            "uninstalled residual obligations must remain rejected"
        );
    }
}
