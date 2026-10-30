use etas_code::{
    contract::{
        TraceInclusion, TraceProofError, TraceProofLimits, canonical_effect, check_trace_inclusion,
    },
    descriptor::{DescriptorGraph, DeterminismInfo, EffectRowInfoRef, TraceInfo, TraceInfoRef},
};

use crate::{
    CheckedProgram,
    code::{definition::DefinitionCatalog, descriptor::ProjectionError, draft::DefaultBodyOverlay},
};

use super::latent::has_latent_output;
use super::{
    CallableContractCheck, CandidateContractAnalysis, ContractComponent as C,
    ContractRejection as Reject, ContractRelation as R, ContractUncertainty as Unknown,
};

pub(crate) fn analyze(
    overlay: &DefaultBodyOverlay,
    candidate: &CheckedProgram,
    descriptors: (
        &DefinitionCatalog,
        Option<&crate::code::definition::FrozenDefinitionContract>,
    ),
    snapshot: etas_code::CodeFingerprint,
    trace_limits: TraceProofLimits,
    descriptor_limits: etas_code::descriptor::DescriptorLimits,
    completed_baseline: Option<&crate::code::pipeline::baseline::CompletedBaseline>,
) -> Result<CandidateContractAnalysis, ProjectionError> {
    let (catalog, applied_candidate) = descriptors;
    let frozen = overlay.frozen();
    let preservation = super::preservation::BodyPreservation::verify(overlay, candidate)?;
    let baseline = frozen.contract();
    if baseline.entry().and_then(|entry| catalog.item_owner(entry)) != candidate.entry {
        return Err(ProjectionError::Invalid(
            "candidate entry is not the canonical frozen default method".into(),
        ));
    }
    let expected_graph = baseline.descriptors();
    let actual_graph =
        applied_candidate.map_or_else(|| catalog.descriptors(), |applied| applied.descriptors());
    let mut checks = Vec::new();
    for (id, expected) in baseline.callables() {
        let completed = completed_baseline.and_then(|baseline| {
            baseline
                .catalog()
                .callable(id)
                .map(|callable| (baseline.catalog().descriptors(), callable))
        });
        let mut record = |component, relation| {
            checks.push(CallableContractCheck {
                callable: id.clone(),
                component,
                relation,
            })
        };
        let actual = match applied_candidate {
            Some(applied) => applied
                .callables()
                .find_map(|(key, callable)| (key == id).then_some(callable)),
            None => catalog.callable(id),
        };
        let Some(actual) = actual else {
            record(C::CallableInterface, R::Rejected(Reject::ChangedInterface));
            continue;
        };
        let preserved = preservation.preserves(id, catalog)?;
        record(
            C::CallableInterface,
            if expected_graph.type_fingerprint(expected.signature)?
                == actual_graph.type_fingerprint(actual.signature)?
            {
                R::Proven
            } else {
                R::Rejected(Reject::ChangedInterface)
            },
        );
        record(
            C::EscapingEffects,
            if preserved {
                R::Proven
            } else {
                let (graph, row) = baseline_fact(
                    expected_graph,
                    expected
                        .analysis
                        .effects
                        .declared_bound
                        .or(expected.analysis.effects.escaping),
                    completed.map(|(graph, callable)| (graph, callable.analysis.effects.escaping)),
                );
                row_relation(graph, row, actual_graph, actual.analysis.effects.escaping)?
            },
        );
        record(
            C::LatentBehavior,
            if preserved {
                R::Proven
            } else if has_latent_output(expected_graph, expected.signature)?
                || has_latent_output(actual_graph, actual.signature)?
            {
                let actual_owner = catalog.item_owner(id).ok_or_else(|| {
                    ProjectionError::MissingIdentity("candidate callable owner".into())
                })?;
                if super::latent::preserves_frozen_origin(
                    &preservation,
                    id,
                    expected_graph,
                    expected.signature,
                    actual_owner,
                    descriptor_limits,
                )? {
                    R::Proven
                } else {
                    R::Unproven(Unknown::MissingLatentBehaviorSummary)
                }
            } else {
                R::Proven
            },
        );
        record(
            C::RequestedActions,
            if preserved {
                R::Proven
            } else {
                let (graph, row) = baseline_fact(
                    expected_graph,
                    expected.analysis.effects.requested,
                    completed.map(|(graph, callable)| (graph, callable.analysis.effects.requested)),
                );
                let relation =
                    row_relation(graph, row, actual_graph, actual.analysis.effects.requested)?;
                if relation != R::Proven {
                    relation
                } else {
                    let expected = match baseline.request_instances(id) {
                        Some(instances) => Some(instances.clone()),
                        None => completed_baseline
                            .map(|baseline| {
                                crate::code::definition::callable_request_instances(
                                    baseline.catalog(),
                                    id,
                                )
                            })
                            .transpose()?
                            .flatten(),
                    };
                    let actual = match applied_candidate {
                        Some(applied) => applied.request_instances(id).cloned(),
                        None => crate::code::definition::callable_request_instances(catalog, id)?,
                    };
                    match (expected, actual) {
                        (None, _) => R::Unproven(Unknown::MissingFrozenAnalysis),
                        (_, None) => R::Unproven(Unknown::MissingCandidateAnalysis),
                        (Some(expected), Some(actual)) if actual.is_subset(&expected) => R::Proven,
                        _ => R::Rejected(Reject::ChangedRequestInstantiation),
                    }
                }
            },
        );
        record(
            C::Determinism,
            // A widened callback footprint does not preserve call order or
            // multiplicity, even when the enclosing source body is identical.
            if preserved
                && !((expected.analysis.determinism.is_none()
                    || actual.analysis.determinism.is_none())
                    && (expected.analysis.trace.is_some_and(|trace| {
                        trace_has_widened_parameter_calls(expected_graph, trace)
                    }) || actual.analysis.trace.is_some_and(|trace| {
                        trace_has_widened_parameter_calls(actual_graph, trace)
                    })))
            {
                R::Proven
            } else {
                let expected_determinism = expected
                    .analysis
                    .determinism
                    .or_else(|| completed.and_then(|(_, callable)| callable.analysis.determinism));
                match (expected_determinism, actual.analysis.determinism) {
                    (Some(expected), Some(actual)) => {
                        if determinism(actual) <= determinism(expected) {
                            R::Proven
                        } else {
                            R::Rejected(Reject::WeakenedDeterminism)
                        }
                    }
                    _ => {
                        let (graph, expected_trace) = baseline_fact(
                            expected_graph,
                            expected.analysis.trace,
                            completed.map(|(graph, callable)| (graph, callable.analysis.trace)),
                        );
                        let independent = if expected.analysis.trace.is_some() {
                            expected.analysis.independent_determinism
                        } else {
                            completed
                                .and_then(|(_, callable)| callable.analysis.independent_determinism)
                        };
                        match (
                            expected_trace,
                            actual.analysis.trace,
                            independent,
                            actual.analysis.independent_determinism,
                        ) {
                            (
                                Some(expected_trace),
                                Some(actual_trace),
                                Some(expected),
                                Some(actual),
                            ) if trace_has_parameter_calls(graph, expected_trace)
                                || trace_has_parameter_calls(actual_graph, actual_trace) =>
                            {
                                if determinism(actual) > determinism(expected) {
                                    R::Rejected(Reject::WeakenedDeterminism)
                                } else {
                                    match check_trace_inclusion(
                                        actual_graph,
                                        actual_trace,
                                        graph,
                                        expected_trace,
                                        trace_limits,
                                    ) {
                                        Ok(TraceInclusion::Included) => R::Proven,
                                        Ok(TraceInclusion::NotProven(_)) => {
                                            R::Unproven(Unknown::UnprovenParametricDeterminism)
                                        }
                                        Err(TraceProofError::Limit { resource, limit }) => {
                                            R::Unproven(Unknown::TraceProofLimit {
                                                resource,
                                                limit,
                                            })
                                        }
                                        Err(error) => {
                                            return Err(ProjectionError::Invalid(
                                                error.to_string(),
                                            ));
                                        }
                                    }
                                }
                            }
                            _ => R::Unproven(Unknown::UnprovenParametricDeterminism),
                        }
                    }
                }
            },
        );
        record(
            C::ActionTrace,
            if preserved {
                R::Proven
            } else {
                let (graph, trace) = baseline_fact(
                    expected_graph,
                    expected.analysis.trace,
                    completed.map(|(graph, callable)| (graph, callable.analysis.trace)),
                );
                match (trace, actual.analysis.trace) {
                    (None, _) => R::Unproven(Unknown::MissingFrozenAnalysis),
                    (_, None) => R::Unproven(Unknown::MissingCandidateAnalysis),
                    (Some(expected), Some(actual)) => match check_trace_inclusion(
                        actual_graph,
                        actual,
                        graph,
                        expected,
                        trace_limits,
                    ) {
                        Ok(TraceInclusion::Included) => R::Proven,
                        Ok(TraceInclusion::NotProven(witness)) => {
                            R::Rejected(Reject::ActionTraceNotContained(witness))
                        }
                        Err(TraceProofError::Limit { resource, limit }) => {
                            R::Unproven(Unknown::TraceProofLimit { resource, limit })
                        }
                        Err(error) => return Err(ProjectionError::Invalid(error.to_string())),
                    },
                }
            },
        );
        record(
            C::TemporalOutcomes,
            if preserved {
                R::Proven
            } else {
                let (graph, outcomes) = baseline_fact(
                    expected_graph,
                    expected.analysis.outcomes,
                    completed.map(|(graph, callable)| (graph, callable.analysis.outcomes)),
                );
                match (outcomes, actual.analysis.outcomes) {
                    (Some(expected), Some(actual)) => match check_trace_inclusion(
                        actual_graph,
                        actual,
                        graph,
                        expected,
                        trace_limits,
                    ) {
                        Ok(TraceInclusion::Included) => R::Proven,
                        Ok(TraceInclusion::NotProven(witness)) => {
                            R::Rejected(Reject::OutcomeTraceNotContained(witness))
                        }
                        Err(TraceProofError::Limit { resource, limit }) => {
                            R::Unproven(Unknown::TraceProofLimit { resource, limit })
                        }
                        Err(error) => return Err(ProjectionError::Invalid(error.to_string())),
                    },
                    _ => R::Unproven(Unknown::MissingOutcomeSensitiveSummary),
                }
            },
        );
        let item = catalog
            .item_owner(id)
            .ok_or_else(|| ProjectionError::MissingIdentity("candidate callable owner".into()))?;
        record(
            C::ResidualChecks,
            match candidate.effects.item_effects.get(&item) {
                None => R::Unproven(Unknown::MissingCandidateAnalysis),
                Some(summary) if summary.residual_checks.checks.is_empty() => R::Proven,
                Some(_) => R::Unproven(Unknown::UnsupportedResidualChecks),
            },
        );
    }
    Ok(CandidateContractAnalysis::Analyzed {
        snapshot,
        frozen_contract: baseline.fingerprint(),
        checks,
    })
}

fn baseline_fact<'a, T: Copy>(
    frozen_graph: &'a DescriptorGraph,
    frozen: Option<T>,
    completed: Option<(&'a DescriptorGraph, Option<T>)>,
) -> (&'a DescriptorGraph, Option<T>) {
    match (frozen, completed) {
        (Some(value), _) => (frozen_graph, Some(value)),
        (None, Some(value)) => value,
        (None, None) => (frozen_graph, None),
    }
}

fn determinism(value: DeterminismInfo) -> u8 {
    match value {
        DeterminismInfo::Deterministic => 0,
        DeterminismInfo::RuntimeMediated => 1,
        DeterminismInfo::NonDeterministic => 2,
    }
}

fn trace_has_parameter_calls(graph: &DescriptorGraph, root: TraceInfoRef) -> bool {
    let mut pending = vec![root];
    let mut visited = std::collections::HashSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }
        match &graph.data().traces[current.0 as usize] {
            TraceInfo::ParameterCall(_)
            | TraceInfo::Outcome(etas_code::descriptor::CallableOutcomeInfo::ParameterNonReturn(
                _,
            )) => return true,
            TraceInfo::Widened {
                parameter_calls, ..
            } if !parameter_calls.is_empty() => return true,
            TraceInfo::Seq(parts) | TraceInfo::Choice(parts) => {
                pending.extend(parts.iter().copied())
            }
            TraceInfo::Repeat(inner) => pending.push(*inner),
            TraceInfo::Empty
            | TraceInfo::Event(_)
            | TraceInfo::Outcome(_)
            | TraceInfo::UnknownOrder(_)
            | TraceInfo::Widened { .. } => {}
        }
    }
    false
}

fn trace_has_widened_parameter_calls(graph: &DescriptorGraph, root: TraceInfoRef) -> bool {
    let mut pending = vec![root];
    let mut visited = std::collections::HashSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current) {
            continue;
        }
        match &graph.data().traces[current.0 as usize] {
            TraceInfo::Widened {
                parameter_calls, ..
            } if !parameter_calls.is_empty() => return true,
            TraceInfo::Seq(parts) | TraceInfo::Choice(parts) => {
                pending.extend(parts.iter().copied())
            }
            TraceInfo::Repeat(inner) => pending.push(*inner),
            _ => {}
        }
    }
    false
}

fn row_relation(
    expected_graph: &DescriptorGraph,
    expected: Option<EffectRowInfoRef>,
    actual_graph: &DescriptorGraph,
    actual: Option<EffectRowInfoRef>,
) -> Result<R, ProjectionError> {
    let Some(expected) = expected else {
        return Ok(R::Unproven(Unknown::MissingFrozenAnalysis));
    };
    let Some(actual) = actual else {
        return Ok(R::Unproven(Unknown::MissingCandidateAnalysis));
    };
    let expected = expected_graph
        .effect_row(expected)
        .ok_or_else(|| ProjectionError::Invalid("missing frozen contract effect row".into()))?;
    let actual = actual_graph
        .effect_row(actual)
        .ok_or_else(|| ProjectionError::Invalid("missing candidate contract effect row".into()))?;
    if let Some(actual) = actual.tail {
        let actual = actual_graph
            .binder(actual)
            .ok_or_else(|| ProjectionError::Invalid("missing candidate effect binder".into()))?;
        let expected = expected.tail.and_then(|tail| expected_graph.binder(tail));
        if !expected.is_some_and(|expected| {
            expected.owner == actual.owner
                && expected.position == actual.position
                && expected.kind == actual.kind
        }) {
            return Ok(R::Rejected(Reject::AdditionalEffects));
        }
    }
    let expected = expected
        .effects
        .iter()
        .map(|effect| canonical_effect(expected_graph, effect))
        .collect::<Result<Vec<_>, _>>()?;
    for effect in &actual.effects {
        let effect = canonical_effect(actual_graph, effect)?;
        if !expected.iter().any(|expected| expected.covers(&effect)) {
            return Ok(R::Rejected(Reject::AdditionalEffects));
        }
    }
    Ok(R::Proven)
}
