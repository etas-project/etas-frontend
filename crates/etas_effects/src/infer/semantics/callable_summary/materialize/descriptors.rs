use crate::infer::semantics::{
    callable_reference::environment, callable_summary::CallableSummaryMode,
    engine::EffectSemantics, specialize::EffectSpecializationError, state::LatentSource,
};
use crate::{CallableReferenceTarget, CallableReturnReference, EffectUnit};
use crate::{
    CallableReturnSource, EffectPipelineError,
    facts::{
        CallableEnvironmentId, CallableReferenceAnalysis, CapturedCallableAnalysis,
        CapturedCallableId,
    },
};
use etas_hir::SymbolDef;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Descriptor {
    Captured(CapturedCallableId),
    Reference(CallableReturnReference),
}

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    pub(crate) fn materialize_callable_descriptors(
        &mut self,
        captures: BTreeSet<CapturedCallableId>,
    ) -> Result<(), EffectPipelineError> {
        let mut instances = captures
            .into_iter()
            .map(Descriptor::Captured)
            .chain(
                self.callable_references
                    .keys()
                    .cloned()
                    .map(Descriptor::Reference),
            )
            .collect::<BTreeSet<_>>();
        let mut visited_origins = BTreeSet::new();
        for summary in self.inputs.unit_effects.values() {
            self.collect_return_descriptors(summary, None, &mut instances, &mut visited_origins)?;
        }
        for snapshot in self.callable_environments.borrow().values() {
            for source in snapshot.parameters.values().flatten() {
                self.collect_origin_descriptors(*source, &mut instances, &mut visited_origins)?;
            }
        }
        for inputs in self
            .parameter_projection_index
            .borrow()
            .call_inputs
            .values()
        {
            for argument in &inputs.arguments {
                for origin in argument.sources.iter().chain(
                    argument
                        .projections
                        .iter()
                        .flat_map(|projection| &projection.sources),
                ) {
                    self.collect_origin_descriptors(*origin, &mut instances, &mut visited_origins)?;
                }
            }
        }
        let mut pending = instances.iter().cloned().collect::<Vec<_>>();
        let mut references = BTreeMap::new();
        let mut captures = BTreeMap::new();
        while let Some(descriptor) = pending.pop() {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            if instances.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(EffectPipelineError::AnalysisLimitExceeded {
                    resource: "callable descriptor nodes",
                    limit: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
                });
            }
            let mut returned = BTreeSet::new();
            let environment = match &descriptor {
                Descriptor::Captured(instance) => Some(instance.environment),
                Descriptor::Reference(reference) => reference.environment,
            };
            if let Some(environment) =
                environment.and_then(|id| self.callable_environments.borrow().get(&id).cloned())
            {
                for origin in environment.parameters.values().flatten() {
                    self.collect_origin_descriptors(*origin, &mut returned, &mut visited_origins)?;
                }
            }
            match descriptor {
                Descriptor::Reference(reference) => {
                    let analysis = self.materialize_reference(&reference)?;
                    if let CallableReferenceAnalysis::Solved {
                        summary,
                        invocation_environment,
                        ..
                    } = &analysis
                    {
                        self.collect_return_descriptors(
                            summary,
                            *invocation_environment,
                            &mut returned,
                            &mut visited_origins,
                        )?;
                    }
                    references.insert(reference, analysis);
                }
                Descriptor::Captured(instance) => {
                    let span = match instance.unit {
                        EffectUnit::Item(item) => self
                            .hir
                            .items
                            .get(item)
                            .ok_or_else(|| EffectPipelineError::InvalidCallableFacts {
                                reason: "captured declaration is missing".into(),
                            })?
                            .span(),
                        EffectUnit::AnonymousFlow { value, .. } => self
                            .hir
                            .exprs
                            .get(value)
                            .ok_or(EffectPipelineError::MissingExprFact { expr: value })?
                            .span(&self.hir.blocks),
                        _ => {
                            return Err(EffectPipelineError::InvalidCallableFacts {
                                reason: "captured descriptor has no source declaration".into(),
                            });
                        }
                    };
                    let analysis = match self.solve_callable_summary_in_mode(
                        LatentSource::Captured {
                            unit: instance.unit,
                            environment: instance.environment,
                        },
                        span,
                        CallableSummaryMode::Descriptor,
                    ) {
                        Ok(summary) => {
                            self.collect_return_descriptors(
                                &summary,
                                Some(instance.environment),
                                &mut returned,
                                &mut visited_origins,
                            )?;
                            CapturedCallableAnalysis::Solved(Arc::new(summary))
                        }
                        Err(EffectSpecializationError::Interrupted(interruption)) => {
                            return Err(EffectPipelineError::Interrupted(interruption));
                        }
                        Err(error) => CapturedCallableAnalysis::Unavailable {
                            reason: error.to_string(),
                        },
                    };
                    captures.insert(instance, analysis);
                }
            }
            for nested in returned {
                if instances.insert(nested.clone()) {
                    pending.push(nested);
                }
            }
        }
        self.inputs.callable_references = references;
        self.inputs.captured_callables = captures;
        Ok(())
    }

    fn collect_return_descriptors(
        &self,
        summary: &crate::EffectSummary,
        environment: Option<CallableEnvironmentId>,
        output: &mut BTreeSet<Descriptor>,
        visited: &mut BTreeSet<LatentSource>,
    ) -> Result<(), EffectPipelineError> {
        collect(summary, environment, output);
        for source in super::descriptor_return_sources(summary) {
            if let CallableReturnSource::Projection(id) = source {
                self.collect_origin_descriptors(LatentSource::Projection(id), output, visited)?;
            }
        }
        Ok(())
    }

    fn collect_origin_descriptors(
        &self,
        origin: LatentSource,
        output: &mut BTreeSet<Descriptor>,
        seen: &mut BTreeSet<LatentSource>,
    ) -> Result<(), EffectPipelineError> {
        let mut pending = vec![origin];
        while let Some(origin) = pending.pop() {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            if !seen.insert(origin) {
                continue;
            }
            match origin {
                LatentSource::Captured { unit, environment }
                | LatentSource::FrozenContract {
                    unit,
                    environment: Some(environment),
                    reference: None,
                } => {
                    output.insert(Descriptor::Captured(CapturedCallableId {
                        unit,
                        environment,
                    }));
                }
                LatentSource::Projection(id) => {
                    if let Some(projection) = self.parameter_projection(id) {
                        for inputs in projection.call_inputs.values() {
                            for argument in &inputs.arguments {
                                pending.extend(&argument.sources);
                                pending.extend(
                                    argument
                                        .projections
                                        .iter()
                                        .flat_map(|projection| &projection.sources),
                                );
                            }
                        }
                    }
                }
                _ => {
                    if let Some(reference) = self.callable_reference_for_source(origin) {
                        output.insert(Descriptor::Reference(reference));
                    }
                }
            }
        }
        Ok(())
    }

    fn materialize_reference(
        &self,
        reference: &CallableReturnReference,
    ) -> Result<CallableReferenceAnalysis, EffectPipelineError> {
        let mut key = reference.clone();
        key.environment = None;
        let source = *self.callable_references.get(&key).ok_or_else(|| {
            EffectPipelineError::InvalidCallableFacts {
                reason: "exported reference has no checked declaration fact".into(),
            }
        })?;
        let source =
            environment::with_environment(source, reference.environment).map_err(pipeline_error)?;
        let span = self
            .hir
            .exprs
            .get(reference.value)
            .ok_or(EffectPipelineError::MissingExprFact {
                expr: reference.value,
            })?
            .span(&self.hir.blocks);
        let summary = match self.reference_summary(source, span) {
            Ok(summary) => summary,
            Err(EffectSpecializationError::Interrupted(interruption)) => {
                return Err(EffectPipelineError::Interrupted(interruption));
            }
            Err(error) => {
                return Ok(CallableReferenceAnalysis::Unavailable {
                    reason: error.to_string(),
                });
            }
        };
        let (types, rows) = self
            .reference_environment_bindings(reference)
            .map_err(pipeline_error)?;
        let invocation_environment = self
            .reference_invocation_environment(source)
            .map_err(pipeline_error)?;
        let (param_names, param_symbols) = self
            .reference_parameters(reference)
            .map_err(pipeline_error)?;
        Ok(CallableReferenceAnalysis::Solved {
            summary: Arc::new(summary),
            type_bindings: types,
            effect_row_bindings: rows,
            invocation_environment,
            param_names,
            param_symbols,
        })
    }

    fn reference_parameters(
        &self,
        reference: &CallableReturnReference,
    ) -> Result<(Vec<String>, Vec<etas_hir::SymbolId>), EffectSpecializationError> {
        match &reference.target {
            CallableReferenceTarget::Source(item) => {
                let params = self.unit_params(EffectUnit::Item(*item)).ok_or_else(|| {
                    invalid("source reference has no checked parameter interface")
                })?;
                let names = params
                    .iter()
                    .map(|symbol| {
                        self.hir
                            .symbols
                            .get(*symbol)
                            .map(|data| data.name.clone())
                            .ok_or_else(|| invalid("source reference parameter symbol is missing"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((names, params))
            }
            CallableReferenceTarget::StdIntrinsic(intrinsic) => {
                let fact = self
                    .inputs
                    .std_callable_values
                    .get(&reference.value)
                    .ok_or_else(|| invalid("standard reference has no checked descriptor"))?;
                if *intrinsic != fact.intrinsic {
                    return Err(invalid(
                        "standard reference intrinsic identity does not match",
                    ));
                }
                let Some(SymbolDef::ImportAlias { path, .. }) =
                    self.hir.symbols.get(fact.symbol).map(|symbol| &symbol.def)
                else {
                    return Err(invalid(
                        "standard reference has no resolved canonical import",
                    ));
                };
                let Some(declaration) = self.std_registry.lookup_qualified(path) else {
                    return Err(invalid(
                        "standard reference declaration is absent from the registry",
                    ));
                };
                if declaration
                    .intrinsic
                    .as_ref()
                    .is_none_or(|descriptor| descriptor.id.0 != *intrinsic)
                {
                    return Err(invalid(
                        "standard reference descriptor has a different intrinsic identity",
                    ));
                }
                let Some(etas_types::SymbolTypeFact::Flow { signature }) = self
                    .type_symbols
                    .symbol_fact(self.hir, &self.types.facts, fact.symbol)
                else {
                    return Err(invalid(
                        "standard reference descriptor has no checked callable signature",
                    ));
                };
                // Std declarations expose positional parameters, not source names.
                Ok((
                    (0..signature.params.len())
                        .map(|position| format!("arg{position}"))
                        .collect(),
                    vec![],
                ))
            }
            CallableReferenceTarget::External {
                callable,
                reflection,
                ..
            } => {
                let metadata = if *reflection {
                    self.external_reflection_metadata(reference.value)
                } else {
                    self.external_reference_metadata(reference.value)
                }
                .ok_or_else(|| invalid("external reference has no checked metadata"))?;
                if metadata
                    .callable_contract
                    .as_ref()
                    .is_none_or(|contract| &contract.header.id != callable)
                {
                    return Err(invalid(
                        "external reference has a different callable identity",
                    ));
                }
                Ok((metadata.param_names.clone(), vec![]))
            }
        }
    }
}

fn pipeline_error(error: EffectSpecializationError) -> EffectPipelineError {
    match error {
        EffectSpecializationError::Interrupted(interruption) => {
            EffectPipelineError::Interrupted(interruption)
        }
        other => EffectPipelineError::InvalidCallableFacts {
            reason: other.to_string(),
        },
    }
}

fn collect(
    summary: &crate::EffectSummary,
    environment: Option<CallableEnvironmentId>,
    instances: &mut BTreeSet<Descriptor>,
) {
    for source in super::descriptor_return_sources(summary) {
        match source {
            CallableReturnSource::Reference(mut reference) => {
                reference.environment = reference.environment.or(environment);
                instances.insert(Descriptor::Reference(reference));
            }
            CallableReturnSource::Captured { unit, environment }
            | CallableReturnSource::FrozenContract {
                unit,
                reference: None,
                environment: Some(environment),
            } => {
                instances.insert(Descriptor::Captured(CapturedCallableId {
                    unit,
                    environment,
                }));
            }
            CallableReturnSource::AnonymousFlow(unit) => {
                if let Some(environment) = environment {
                    instances.insert(Descriptor::Captured(CapturedCallableId {
                        unit,
                        environment,
                    }));
                }
            }
            _ => {}
        }
    }
}
