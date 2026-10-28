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
        for summary in self.inputs.unit_effects.values() {
            collect(summary, None, &mut instances);
        }
        for snapshot in self.callable_environments.borrow().values() {
            for source in snapshot.parameters.values().flatten() {
                if let Some(reference) = self.callable_reference_for_source(*source) {
                    instances.insert(Descriptor::Reference(reference));
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
            match descriptor {
                Descriptor::Reference(reference) => {
                    let analysis = self.materialize_reference(&reference)?;
                    if let CallableReferenceAnalysis::Solved {
                        summary,
                        invocation_environment,
                        ..
                    } = &analysis
                    {
                        collect(summary, *invocation_environment, &mut returned);
                    }
                    references.insert(reference, analysis);
                }
                Descriptor::Captured(instance) => {
                    let EffectUnit::AnonymousFlow { value, .. } = instance.unit else {
                        return Err(EffectPipelineError::InvalidCallableFacts {
                            reason: "captured descriptor is not an anonymous source unit".into(),
                        });
                    };
                    let span = self
                        .hir
                        .exprs
                        .get(value)
                        .ok_or(EffectPipelineError::MissingExprFact { expr: value })?
                        .span(&self.hir.blocks);
                    let analysis = match self.solve_callable_summary_in_mode(
                        LatentSource::Captured {
                            unit: instance.unit,
                            environment: instance.environment,
                        },
                        span,
                        CallableSummaryMode::Descriptor,
                    ) {
                        Ok(summary) => {
                            collect(&summary, Some(instance.environment), &mut returned);
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
        match reference.target {
            CallableReferenceTarget::Source(item) => {
                let params = self.unit_params(EffectUnit::Item(item)).ok_or_else(|| {
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
                if intrinsic != fact.intrinsic {
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
                    .is_none_or(|descriptor| descriptor.id.0 != intrinsic)
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
    for source in summary
        .exits
        .as_ref()
        .and_then(|exits| exits.callable_return_sources())
        .into_iter()
        .flatten()
    {
        match source {
            CallableReturnSource::Reference(mut reference) => {
                reference.environment = reference.environment.or(environment);
                instances.insert(Descriptor::Reference(reference));
            }
            CallableReturnSource::Captured { unit, environment } => {
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
