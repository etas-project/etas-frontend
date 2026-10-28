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
                self.prepare_callable_summary(key)?
            };
            let outgoing = plan.dependencies(mode)?;
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
                    let mut summary = self.apply_callable_summary(plan, &summaries, mode)?;
                    summary.stabilize_for_fixpoint(None);
                    summaries.insert(key, summary);
                }
                continue;
            }
            let members = component.nodes.iter().copied().collect::<BTreeSet<_>>();
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
                    let candidate = self.apply_callable_summary(plan, summaries, mode)?;
                    let mut joined = previous.clone();
                    joined.join_summary(&candidate);
                    joined.stabilize_for_fixpoint(Some(previous));
                    joined.action_trace = joined.action_trace.repeat().stabilize_for_fixpoint(None);
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
            CallableSummaryPlan::Captured(plan) => plan,
            CallableSummaryPlan::Leaf(summary) => return Ok(summary.clone()),
        };
        let mut summary = plan.template.clone();
        let requests = match &plan.template.handler_requests {
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
                        parameters, ..
                    } = &mut requests
                    {
                        parameters.insert(parameter.clone());
                    }
                }
                requests
            }
        };
        summary.action_trace =
            substitute_parameter_calls(&plan.template.action_trace, |target, _, span| {
                let Some(origins) = plan.bindings.get(&target) else {
                    return Ok(ParameterCallSubstitution::Symbolic);
                };
                let captured = joined_origins(origins, span, summaries, mode)?;
                summary.merge_request_footprint_without_escaping_or_trace(&captured);
                Ok(ParameterCallSubstitution::Trace(captured.action_trace))
            })?;
        summary.handler_requests = requests;
        self.solve_callback_completion_with(&mut summary, &plan.type_bindings, |target, span| {
            plan.bindings
                .get(&target)
                .map(|origins| joined_origins(origins, span, summaries, mode))
                .transpose()
        })?;
        Ok(summary)
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
