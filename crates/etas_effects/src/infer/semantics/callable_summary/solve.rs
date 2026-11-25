use etas_utils::{ConvergenceStatus, FixpointEngine, GraphView, IterationLimit};

use super::*;
use crate::infer::semantics::{
    engine::EffectSemantics,
    specialize::{ParameterCallSubstitution, substitute_parameter_calls},
};

impl EffectSemantics<'_> {
    pub(crate) fn solve_callable_summary(
        &self,
        source: LatentSource,
        span: Span,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        self.solve_callable_summary_in_mode(source, span, CallableSummaryMode::Invocation)
    }

    pub(in crate::infer::semantics) fn solve_callable_summary_in_mode(
        &self,
        source: LatentSource,
        span: Span,
        mode: CallableSummaryMode,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        self.callable_summary_checkpoint()?;
        let root = CallableSummaryKey::in_mode(source, span, mode);
        if let Some(summary) = self.callable_summaries.borrow().summaries.get(&root) {
            return Ok(summary.clone());
        }
        let mut plans = BTreeMap::new();
        let mut dependents = BTreeMap::<CallableSummaryKey, BTreeSet<CallableSummaryKey>>::new();
        let mut dependencies = BTreeMap::new();
        let mut pending = vec![root];
        let mut edges = 0usize;
        while let Some(key) = pending.pop() {
            self.callable_summary_checkpoint()?;
            if plans.contains_key(&key) {
                continue;
            }
            if plans.len() + self.callable_summaries.borrow().summaries.len()
                >= etas_package_metadata::MAX_METADATA_GRAPH_NODES
            {
                return Err(invalid(
                    "callable summary graph exceeds the analysis node budget",
                ));
            }
            let plan = if let Some(summary) = self.callable_summaries.borrow().summaries.get(&key) {
                CallableSummaryPlan::Leaf(summary.clone())
            } else {
                self.prepare_callable_summary(key)
                    .map_err(|error| context(error, "preparing checked callable summary"))?
            };
            let outgoing = plan
                .dependencies(mode)
                .map_err(|error| context(error, "resolving checked callable dependencies"))?;
            edges = edges.saturating_add(outgoing.len());
            if edges > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid(
                    "callable summary graph exceeds the analysis edge budget",
                ));
            }
            for dependency in &outgoing {
                dependents.entry(*dependency).or_default().insert(key);
                pending.push(*dependency);
            }
            dependencies.insert(key, outgoing);
            plans.insert(key, plan);
        }
        let graph = CallableSummaryGraph {
            plans: &plans,
            dependencies: &dependencies,
        };
        let mut summaries = BTreeMap::new();
        // Shared Tarjan emits outgoing dependency components before their users.
        // Acyclic calls therefore never observe a provisional epsilon trace.
        for mut component in etas_utils::strongly_connected_components(&graph) {
            self.callable_summary_checkpoint()?;
            component.nodes.sort();
            let cyclic = component.nodes.len() > 1
                || component.nodes.iter().any(|key| {
                    dependencies
                        .get(key)
                        .is_some_and(|dependencies| dependencies.contains(key))
                });
            if !cyclic {
                for key in component.nodes {
                    let plan = plans
                        .get(&key)
                        .ok_or_else(|| invalid("callable component has no checked plan"))?;
                    let mut summary = self
                        .apply_callable_summary(plan, &summaries, mode)
                        .map_err(|error| context(error, "solving checked callable summary"))?;
                    summary.stabilize_for_fixpoint(None);
                    summary.action_trace = summary.action_trace.shared();
                    if let Some(exits) = &mut summary.exits {
                        for exit in &mut exits.exits {
                            exit.trace = std::mem::take(&mut exit.trace).shared();
                        }
                        for exit in &mut exits.parameter_exits {
                            exit.prefix = std::mem::take(&mut exit.prefix).shared();
                        }
                    }
                    summaries.insert(key, summary);
                }
                continue;
            }
            let members = component.nodes.iter().copied().collect::<BTreeSet<_>>();
            let mut trace_equations = crate::infer::recursive_trace::TraceEquations::default();
            for key in &members {
                summaries.insert(*key, EffectSummary::local());
            }
            let limit = IterationLimit::new(
                IterationLimit::DEFAULT
                    .max_iterations()
                    .saturating_mul(members.len()),
            );
            let solved = FixpointEngine::new(limit).try_solve_worklist(
                summaries,
                component.nodes,
                |key, summaries| {
                    self.callable_summary_checkpoint()?;
                    let plan = plans.get(key).ok_or_else(|| {
                        invalid("callable summary worklist lost its checked plan")
                    })?;
                    let previous = summaries.get(key).ok_or_else(|| {
                        invalid("callable summary worklist lost its solution slot")
                    })?;
                    let mut candidate = self.apply_callable_summary(plan, summaries, mode)?;
                    trace_equations.summarize_with_id(
                        crate::infer::recursive_trace::identity(key),
                        &mut candidate,
                    );
                    let mut joined = previous.clone();
                    joined.join_summary(&candidate);
                    joined.stabilize_for_fixpoint(Some(previous));
                    let changed = &joined != previous;
                    if changed {
                        summaries.insert(*key, joined);
                    }
                    Ok::<_, EffectSpecializationError>(changed)
                },
                |key, _| {
                    dependents.get(key).map_or_else(Vec::new, |parents| {
                        parents
                            .iter()
                            .filter(|parent| members.contains(parent))
                            .copied()
                            .collect()
                    })
                },
            )?;
            if solved.status != ConvergenceStatus::Converged {
                return Err(invalid(
                    "captured callable summaries did not converge within the analysis iteration budget",
                ));
            }
            summaries = solved.value;
            for key in &members {
                if let Some(summary) = summaries.get_mut(key) {
                    trace_equations.close_summary(summary);
                }
            }
        }
        let summary = summaries
            .get(&root)
            .cloned()
            .ok_or_else(|| invalid("solved callable summary graph has no root"))?;
        self.callable_summaries
            .borrow_mut()
            .summaries
            .extend(summaries);
        Ok(summary)
    }

    fn callable_summary_checkpoint(&self) -> Result<(), EffectSpecializationError> {
        if let Some(control) = self.query_control
            && let Err(error) = control.checkpoint()
        {
            self.callable_summaries.borrow_mut().interruption = Some(error.clone());
            return Err(EffectSpecializationError::Interrupted(error));
        }
        Ok(())
    }

    fn apply_callable_summary(
        &self,
        plan: &CallableSummaryPlan,
        summaries: &BTreeMap<CallableSummaryKey, EffectSummary>,
        mode: CallableSummaryMode,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let plan = match plan {
            CallableSummaryPlan::Dispatch {
                implementations,
                managed,
            } => {
                let mut alternatives = implementations.iter().map(|key| {
                    let body = summaries
                        .get(key)
                        .cloned()
                        .ok_or_else(|| invalid("method implementation summary is missing"))?;
                    if *managed {
                        self.agent_request_summary(key.source, key.span(), body)
                    } else {
                        Ok(body)
                    }
                });
                let mut result = alternatives.next().ok_or_else(|| {
                    invalid("method dispatch has no solved implementation alternatives")
                })??;
                for alternative in alternatives {
                    result.join_branch(&alternative?);
                }
                return Ok(result);
            }
            CallableSummaryPlan::Captured(plan) => plan,
            CallableSummaryPlan::Leaf(summary) => return Ok(summary.clone()),
        };
        let mut summary = plan.template.clone();
        let instantiate_requests = |requests: &crate::RequestInstantiationSummary| -> Result<
            crate::RequestInstantiationSummary,
            EffectSpecializationError,
        > {
            Ok(match requests {
                crate::RequestInstantiationSummary::Unavailable => {
                    crate::RequestInstantiationSummary::Unavailable
                }
                crate::RequestInstantiationSummary::Available {
                    actions,
                    parameters,
                } => {
                    let mut requests = crate::RequestInstantiationSummary::Available {
                        actions: actions.clone(),
                        parameters: BTreeSet::new(),
                    };
                    for parameter in parameters {
                        if let Some(origins) = plan.bindings.get(&parameter.target) {
                            let mut captured =
                                joined_origins(origins, parameter.span, summaries, mode)?
                                    .handler_requests;
                            captured.subtract_handled(
                                &crate::EffectCoverage {
                                    registry: self.registry,
                                    types: &self.types.store,
                                },
                                &crate::EffectRow::closed(parameter.excluded.clone()),
                            );
                            requests.union_assign(&captured);
                        } else if let crate::RequestInstantiationSummary::Available {
                            parameters,
                            ..
                        } = &mut requests
                        {
                            parameters.insert(parameter.clone());
                        }
                    }
                    requests
                }
            })
        };
        // A callback can introduce default behavior while its footprint is
        // merged below. That behavior must capture the instantiated requests,
        // rather than the template's implementation-local parameters.
        summary.handler_requests = instantiate_requests(&plan.template.handler_requests)?;
        if let Some(behavior) = &mut summary.default_behavior {
            behavior.direct.handler_requests =
                instantiate_requests(&behavior.direct.handler_requests)?;
            for footprint in behavior.guarded.values_mut() {
                footprint.handler_requests = instantiate_requests(&footprint.handler_requests)?;
            }
        }
        summary.action_trace =
            substitute_parameter_calls(&plan.template.action_trace, |target, _, span| {
                let Some(origins) = plan.bindings.get(&target) else {
                    return Ok(ParameterCallSubstitution::Symbolic);
                };
                let captured = joined_origins(origins, span, summaries, mode)?;
                summary.merge_request_footprint_without_escaping_or_trace(&captured);
                Ok(ParameterCallSubstitution::Trace(captured.action_trace))
            })?;
        if let Some(exits) = &mut summary.exits {
            for exit in &mut exits.exits {
                exit.trace = substitute_parameter_calls(&exit.trace, |target, _, span| {
                    let Some(origins) = plan.bindings.get(&target) else {
                        return Ok(ParameterCallSubstitution::Symbolic);
                    };
                    let captured = joined_origins(origins, span, summaries, mode)?;
                    Ok(ParameterCallSubstitution::Trace(captured.action_trace))
                })?;
            }
            for exit in &mut exits.parameter_exits {
                exit.prefix = substitute_parameter_calls(&exit.prefix, |target, _, span| {
                    let Some(origins) = plan.bindings.get(&target) else {
                        return Ok(ParameterCallSubstitution::Symbolic);
                    };
                    let captured = joined_origins(origins, span, summaries, mode)?;
                    Ok(ParameterCallSubstitution::Trace(captured.action_trace))
                })?;
            }
        }
        self.solve_callback_completion_with(&mut summary, &plan.type_bindings, |target, span| {
            plan.bindings
                .get(&target)
                .map(|origins| joined_origins(origins, span, summaries, mode))
                .transpose()
        })?;
        Ok(summary)
    }
}

fn context(error: EffectSpecializationError, operation: &str) -> EffectSpecializationError {
    match error {
        EffectSpecializationError::InvalidCallableReturnSources => {
            EffectSpecializationError::InvalidCallableEnvironment {
                reason: format!("{operation} requires complete callable return-source facts"),
            }
        }
        other => other,
    }
}

struct CallableSummaryGraph<'a> {
    plans: &'a BTreeMap<CallableSummaryKey, CallableSummaryPlan>,
    dependencies: &'a BTreeMap<CallableSummaryKey, BTreeSet<CallableSummaryKey>>,
}

impl GraphView for CallableSummaryGraph<'_> {
    type Node = CallableSummaryKey;
    fn nodes(&self) -> Vec<Self::Node> {
        self.plans.keys().copied().collect()
    }
    fn successors(&self, node: &Self::Node) -> Vec<Self::Node> {
        self.dependencies
            .get(node)
            .map_or_else(Vec::new, |dependencies| {
                dependencies.iter().copied().collect()
            })
    }
}

fn joined_origins(
    origins: &BTreeSet<LatentSource>,
    span: Span,
    summaries: &BTreeMap<CallableSummaryKey, EffectSummary>,
    mode: CallableSummaryMode,
) -> Result<EffectSummary, EffectSpecializationError> {
    if origins.is_empty() {
        // A checked empty structural payload (None, or an empty sequence) has
        // no callable to execute. Use an epsilon over-approximation for that
        // unreachable callback branch. Missing bindings and empty direct flow
        // arguments are still rejected when building the dependency graph.
        return Ok(EffectSummary::local());
    }
    let mut combined = None::<EffectSummary>;
    for origin in origins {
        let summary = summaries
            .get(&CallableSummaryKey::in_mode(*origin, span, mode))
            .ok_or_else(|| invalid("captured callable dependency has no solution slot"))?;
        match &mut combined {
            Some(combined) => {
                combined.join_summary(summary);
            }
            None => combined = Some(summary.clone()),
        }
    }
    combined.ok_or_else(|| invalid("captured callable has an empty source set"))
}
