//! Hash-consed ordered traces, including scoped recursive equations.
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use etas_code::CodeFingerprint;

use crate::{ActionTraceDomain as Trace, EffectSet};

pub(super) type NodeId = usize;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum Node {
    Leaf(Trace),
    Seq(Vec<NodeId>),
    Choice(Vec<NodeId>),
    Parallel(Vec<NodeId>),
    Repeat(NodeId),
    DefaultHandler {
        guards: EffectSet,
        body: NodeId,
    },
    TaskStart {
        task_site: etas_hir::HirExprId,
        body: NodeId,
    },
    Equation(Option<NodeId>),
}

pub(super) struct StoredNode {
    pub node: Node,
    pub actions: EffectSet,
    pub parameter_calls: bool,
    pub preserve: bool,
}

#[derive(Default)]
pub(super) struct TraceGraph {
    pub nodes: Vec<StoredNode>,
    interned: HashMap<Node, NodeId>,
    equations: BTreeMap<(CodeFingerprint, Arc<BTreeMap<CodeFingerprint, NodeId>>), NodeId>,
    // Retain the Arc alongside its process-local address until this cache dies.
    shared: BTreeMap<usize, (Trace, NodeId)>,
}

impl TraceGraph {
    pub fn compile(&mut self, trace: &Trace) -> NodeId {
        let first_new_node = self.nodes.len();
        type Bindings = Arc<BTreeMap<CodeFingerprint, NodeId>>;
        enum Work<'a> {
            Visit(&'a Trace, Bindings),
            Build(&'a Trace, usize),
            Equation(NodeId),
            Shared(&'a Trace, usize),
        }
        let mut work = vec![Work::Visit(trace, Arc::default())];
        let mut values = Vec::new();
        while let Some(next) = work.pop() {
            match next {
                Work::Visit(trace, bindings) => match trace {
                    Trace::Shared(body) => {
                        let address = Arc::as_ptr(body) as usize;
                        if let Some((_, node)) = self.shared.get(&address) {
                            values.push(*node);
                        } else {
                            work.push(Work::Shared(trace, address));
                            work.push(Work::Visit(&body.body, Arc::default()));
                        }
                    }
                    Trace::Recursive { id, body: None } => {
                        values.push(
                            bindings
                                .get(id)
                                .copied()
                                .unwrap_or_else(|| self.intern(Node::Leaf(trace.clone()))),
                        );
                    }
                    Trace::Recursive {
                        id,
                        body: Some(body),
                    } => {
                        // Include the surrounding lexical equations: equally
                        // named variables in different instances never alias.
                        let key = (trace.structural_identity(), bindings.clone());
                        if let Some(node) = self.equations.get(&key) {
                            values.push(*node);
                            continue;
                        }
                        let node = self.nodes.len();
                        self.nodes.push(StoredNode {
                            node: Node::Equation(None),
                            actions: EffectSet::default(),
                            parameter_calls: false,
                            preserve: false,
                        });
                        self.equations.insert(key, node);
                        let mut nested = (*bindings).clone();
                        nested.insert(*id, node);
                        work.push(Work::Equation(node));
                        work.push(Work::Visit(body, Arc::new(nested)));
                    }
                    Trace::Seq(parts) | Trace::Choice(parts) | Trace::Parallel(parts) => {
                        work.push(Work::Build(trace, parts.len()));
                        work.extend(
                            parts
                                .iter()
                                .rev()
                                .map(|part| Work::Visit(part, bindings.clone())),
                        );
                    }
                    Trace::Repeat(body)
                    | Trace::DefaultHandler { body, .. }
                    | Trace::TaskStart { body, .. } => {
                        work.push(Work::Build(trace, 1));
                        work.push(Work::Visit(body, bindings));
                    }
                    leaf => values.push(self.intern(Node::Leaf(leaf.clone()))),
                },
                Work::Build(trace, count) => {
                    let parts = values.split_off(values.len() - count);
                    let node = match trace {
                        Trace::Seq(_) => Node::Seq(parts),
                        Trace::Choice(_) => {
                            let mut parts = parts;
                            parts.sort_unstable();
                            parts.dedup();
                            Node::Choice(parts)
                        }
                        Trace::Parallel(_) => Node::Parallel(parts),
                        Trace::Repeat(_) => Node::Repeat(parts[0]),
                        Trace::DefaultHandler { guards, .. } => Node::DefaultHandler {
                            guards: guards.clone(),
                            body: parts[0],
                        },
                        Trace::TaskStart { task_site, .. } => Node::TaskStart {
                            task_site: *task_site,
                            body: parts[0],
                        },
                        _ => unreachable!("ordered trace composite"),
                    };
                    values.push(self.intern(node));
                }
                Work::Equation(node) => {
                    let body = values.pop().expect("compiled equation body");
                    self.nodes[node].node = Node::Equation(Some(body));
                    values.push(node);
                }
                Work::Shared(trace, address) => {
                    let node = *values.last().expect("shared trace root");
                    self.shared.insert(address, (trace.clone(), node));
                }
            }
        }
        self.solve_footprints(first_new_node);
        values.pop().expect("compiled trace root")
    }

    fn intern(&mut self, node: Node) -> NodeId {
        if let Some(id) = self.interned.get(&node) {
            return *id;
        }
        let (actions, parameter_calls) = match &node {
            Node::Leaf(trace) => (
                trace.action_set(),
                trace.has_parameter_calls() || matches!(trace, Trace::Recursive { .. }),
            ),
            _ => (EffectSet::default(), false),
        };
        let id = self.nodes.len();
        let preserve = matches!(
            &node,
            Node::Leaf(Trace::TaskAwait { .. })
                | Node::DefaultHandler { .. }
                | Node::TaskStart { .. }
                | Node::Parallel(_)
        ) || matches!(&node, Node::Choice(parts) if parts.is_empty());
        self.interned.insert(node.clone(), id);
        self.nodes.push(StoredNode {
            node,
            actions,
            parameter_calls,
            preserve,
        });
        id
    }

    fn solve_footprints(&mut self, first_new_node: NodeId) {
        // Published nodes never acquire new edges. Only the appended subgraph
        // needs solving; its recursive equations can read already-solved nodes.
        // This remains a finite union problem when new equation edges cycle.
        loop {
            let mut changed = false;
            for id in (first_new_node..self.nodes.len()).rev() {
                let children = match &self.nodes[id].node {
                    Node::Seq(parts) | Node::Choice(parts) | Node::Parallel(parts) => parts.clone(),
                    Node::Repeat(body)
                    | Node::DefaultHandler { body, .. }
                    | Node::TaskStart { body, .. }
                    | Node::Equation(Some(body)) => vec![*body],
                    _ => Vec::new(),
                };
                for child in children {
                    let actions = self.nodes[child].actions.clone();
                    let calls = self.nodes[child].parameter_calls;
                    let preserve = self.nodes[child].preserve;
                    changed |= self.nodes[id].actions.union_assign(&actions);
                    changed |= calls && !self.nodes[id].parameter_calls;
                    self.nodes[id].parameter_calls |= calls;
                    changed |= preserve && !self.nodes[id].preserve;
                    self.nodes[id].preserve |= preserve;
                }
            }
            if !changed {
                break;
            }
        }
    }
}
