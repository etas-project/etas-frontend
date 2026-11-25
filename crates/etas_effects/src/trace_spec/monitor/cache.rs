//! One checked program owns a cache of finite transfer rows for each concrete
//! spec. Function summaries use the canonical ordered trace root as identity.
use std::collections::BTreeMap;

use etas_code::CodeFingerprint;

use super::{
    graph::{NodeId, TraceGraph},
    runner::{TemporalMonitorResult, TransitionCache, require_before_graph},
};
use crate::trace_spec::model::TraceSpecPattern;
use crate::{ActionTraceDomain, EffectRegistry};

/// Exact finite-language summary for a closed, action-free trace. Nullability
/// is solved as a least fixed point; a recursive back edge is not epsilon.
#[derive(Default)]
pub struct EpsilonTraceAnalysis {
    graph: TraceGraph,
    nullable: Vec<bool>,
}

impl EpsilonTraceAnalysis {
    pub fn is_epsilon(&mut self, trace: &ActionTraceDomain) -> bool {
        use super::graph::Node;
        if !trace.action_set().is_empty() || trace.has_parameter_calls() {
            return false;
        }
        let root = self.graph.compile(trace);
        let stored = &self.graph.nodes[root];
        if !stored.actions.is_empty() || stored.parameter_calls || stored.preserve {
            return false;
        }
        let first_new_node = self.nullable.len();
        self.nullable.resize(self.graph.nodes.len(), false);
        // Published nodes have immutable edges and already have their least
        // fixed point. New equations can depend on those solved rows, but
        // cannot change them. Reuse them when compiling another function.
        loop {
            let mut changed = false;
            for id in (first_new_node..self.graph.nodes.len()).rev() {
                if self.nullable[id] {
                    continue;
                }
                let stored = &self.graph.nodes[id];
                let nullable = match &stored.node {
                    Node::Leaf(
                        ActionTraceDomain::Empty
                        | ActionTraceDomain::UnknownOrder(_)
                        | ActionTraceDomain::Widened { .. },
                    ) => true,
                    Node::Seq(parts) => parts.iter().all(|part| self.nullable[*part]),
                    Node::Choice(parts) => parts.iter().any(|part| self.nullable[*part]),
                    Node::Repeat(_) => true,
                    Node::Equation(Some(body)) => self.nullable[*body],
                    _ => false,
                };
                if nullable {
                    self.nullable[id] = true;
                    changed = true;
                }
            }
            if !changed {
                return self.nullable[root];
            }
        }
    }
}

#[cfg(test)]
mod epsilon_tests {
    use super::*;
    #[test]
    fn pure_recursive_summaries_require_a_productive_empty_path() {
        let id = CodeFingerprint::of_bytes("test.epsilon-equation", b"f");
        let reference = ActionTraceDomain::Recursive { id, body: None };
        let mut analysis = EpsilonTraceAnalysis::default();
        let pure = ActionTraceDomain::Recursive {
            id,
            body: Some(Box::new(ActionTraceDomain::Choice(vec![
                ActionTraceDomain::Empty,
                ActionTraceDomain::Seq(vec![
                    ActionTraceDomain::Repeat(Box::new(ActionTraceDomain::Empty)),
                    reference.clone(),
                ]),
            ]))),
        };
        assert!(analysis.is_epsilon(&pure));
        let nodes = analysis.graph.nodes.len();
        assert!(analysis.is_epsilon(&pure));
        assert_eq!(analysis.graph.nodes.len(), nodes);
        assert!(analysis.is_epsilon(&ActionTraceDomain::Seq(vec![
            pure.clone(),
            ActionTraceDomain::Empty,
        ])));
        // X = X has no completed finite trace; it cannot become epsilon.
        assert!(!analysis.is_epsilon(&ActionTraceDomain::Recursive {
            id,
            body: Some(Box::new(reference))
        }));
    }
}

#[derive(Default)]
struct SpecSummary {
    transitions: TransitionCache,
    functions: BTreeMap<NodeId, TemporalMonitorResult>,
}

pub(crate) struct TemporalMonitorCache<'a> {
    registry: &'a EffectRegistry,
    types: &'a etas_types::TypeOutput,
    graph: TraceGraph,
    specs: BTreeMap<CodeFingerprint, SpecSummary>,
}

impl<'a> TemporalMonitorCache<'a> {
    pub(crate) fn new(registry: &'a EffectRegistry, types: &'a etas_types::TypeOutput) -> Self {
        Self {
            registry,
            types,
            graph: TraceGraph::default(),
            specs: BTreeMap::new(),
        }
    }

    pub(crate) fn require_before(
        &mut self,
        trace: &ActionTraceDomain,
        guard: &TraceSpecPattern,
        target: &TraceSpecPattern,
    ) -> TemporalMonitorResult {
        let bytes = serde_json::to_vec(&(guard, target)).expect("checked trace spec serializes");
        let spec = CodeFingerprint::of_bytes("etas.effects.temporal-spec.v1", &bytes);
        let root = self.graph.compile(trace);
        let summary = self.specs.entry(spec).or_default();
        if let Some(result) = summary.functions.get(&root) {
            return *result;
        }
        let result = require_before_graph(
            &self.graph,
            root,
            guard,
            target,
            self.registry,
            self.types,
            &mut summary.transitions,
        );
        summary.functions.insert(root, result);
        result
    }
}
