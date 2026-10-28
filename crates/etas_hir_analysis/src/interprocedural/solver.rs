use std::collections::{BTreeMap, BTreeSet};

use etas_utils::{
    ConvergenceStatus, FixpointEngine, FixpointStats, GraphView, JoinSemiLattice, topological_sort,
};

use crate::intraprocedural::{Control, HirAbstractInterpreter, HirAnalysisSemantics};

use super::{
    AnalysisPhase, AnalysisUnit, CallGraph, HirAnalysisBody, InterproceduralDiagnostic,
    InterproceduralSemantics, SummaryStore, UnitContext,
    adapter::{CallGraphCollectionAdapter, SummaryApplicationAdapter},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentConvergence<U>
where
    U: AnalysisUnit,
{
    pub units: Vec<U>,
    pub status: ConvergenceStatus,
    pub stats: FixpointStats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummarySolverResult<U, S>
where
    U: AnalysisUnit,
{
    pub summaries: SummaryStore<U, S>,
    pub call_graph: CallGraph<U>,
    pub convergence: Vec<ComponentConvergence<U>>,
    pub diagnostics: Vec<InterproceduralDiagnostic<U>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SummarySolver {
    engine: FixpointEngine,
}

impl SummarySolver {
    pub fn new(engine: FixpointEngine) -> Self {
        Self { engine }
    }

    pub fn solve<S>(
        &self,
        units: impl IntoIterator<Item = S::Unit>,
        semantics: &mut S,
    ) -> SummarySolverResult<S::Unit, S::Summary>
    where
        S: InterproceduralSemantics
            + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
    {
        self.solve_with_checkpoint(units, semantics, &|| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {})
    }

    pub(super) fn solve_with_checkpoint<S, E>(
        &self,
        units: impl IntoIterator<Item = S::Unit>,
        semantics: &mut S,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<SummarySolverResult<S::Unit, S::Summary>, E>
    where
        S: InterproceduralSemantics
            + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
    {
        checkpoint()?;
        let mut units = units.into_iter().collect::<Vec<_>>();
        units.sort();
        units.dedup();

        let mut diagnostics = Vec::new();
        let call_graph = self.build_call_graph(&units, semantics, checkpoint)?;
        let mut summaries = SummaryStore::new();
        for unit in &units {
            checkpoint()?;
            summaries.ensure_bottom(*unit);
        }

        let components = match ordered_components(&call_graph) {
            Ok(components) => components,
            Err(()) => {
                diagnostics.push(InterproceduralDiagnostic::InvalidCondensationGraph);
                return Ok(SummarySolverResult {
                    summaries,
                    call_graph,
                    convergence: Vec::new(),
                    diagnostics,
                });
            }
        };

        let mut convergence = Vec::new();
        for component in components {
            checkpoint()?;
            let component_set = component.iter().copied().collect::<BTreeSet<_>>();
            let result = self.engine.try_solve_worklist(
                summaries,
                component.clone(),
                |unit, summaries| {
                    checkpoint()?;
                    let changed = self.transfer_unit(
                        *unit,
                        semantics,
                        summaries,
                        &mut diagnostics,
                        checkpoint,
                    )?;
                    checkpoint()?;
                    Ok(changed)
                },
                |unit, _summaries| {
                    call_graph
                        .dependents(*unit)
                        .into_iter()
                        .filter(|dependent| component_set.contains(dependent))
                        .collect()
                },
            )?;
            if result.status == ConvergenceStatus::IterationLimitReached {
                push_unique(
                    &mut diagnostics,
                    InterproceduralDiagnostic::IterationLimitReached {
                        component: component.clone(),
                    },
                );
            }
            convergence.push(ComponentConvergence {
                units: component,
                status: result.status,
                stats: result.stats,
            });
            summaries = result.value;
        }

        checkpoint()?;
        Ok(SummarySolverResult {
            summaries,
            call_graph,
            convergence,
            diagnostics,
        })
    }

    fn build_call_graph<S, E>(
        &self,
        units: &[S::Unit],
        semantics: &mut S,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<CallGraph<S::Unit>, E>
    where
        S: InterproceduralSemantics
            + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
    {
        let mut graph = CallGraph::new();
        for unit in units {
            checkpoint()?;
            graph.add_unit(*unit);
        }

        for unit in units {
            checkpoint()?;
            let body = semantics.body_of(*unit);
            if !body.is_traversable() {
                continue;
            }

            let entry = <S as InterproceduralSemantics>::Domain::bottom();
            let adapter = CallGraphCollectionAdapter::new(semantics, *unit, &mut graph);
            let mut interpreter = HirAbstractInterpreter::new(adapter);
            traverse_body(&mut interpreter, body, entry, checkpoint)?;
            checkpoint()?;
        }

        Ok(graph)
    }

    fn transfer_unit<S, E>(
        &self,
        unit: S::Unit,
        semantics: &mut S,
        summaries: &mut SummaryStore<S::Unit, S::Summary>,
        diagnostics: &mut Vec<InterproceduralDiagnostic<S::Unit>>,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<bool, E>
    where
        S: InterproceduralSemantics
            + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
    {
        let context = UnitContext::new(unit, AnalysisPhase::SolveSummaries);
        let body = semantics.body_of(unit);

        match body {
            HirAnalysisBody::External => {
                let summary = semantics.external_summary(context);
                let summary = semantics.stabilize_summary(context, summary, summaries.get(unit));
                return Ok(summaries.join_summary(unit, summary));
            }
            HirAnalysisBody::Missing => {
                push_unique(diagnostics, InterproceduralDiagnostic::MissingBody { unit });
                let summary = semantics.missing_summary(context);
                let summary = semantics.stabilize_summary(context, summary, summaries.get(unit));
                return Ok(summaries.join_summary(unit, summary));
            }
            HirAnalysisBody::Block(_)
            | HirAnalysisBody::Expr(_)
            | HirAnalysisBody::HandlerArm(_) => {}
        }

        let entry = semantics.begin_unit(context);
        let exit = {
            let adapter = SummaryApplicationAdapter::new(semantics, unit, summaries, diagnostics);
            let mut interpreter = HirAbstractInterpreter::new(adapter);
            traverse_body(&mut interpreter, body, entry, checkpoint)?
        };
        let summary = semantics.end_unit(context, exit);
        let summary = semantics.stabilize_summary(context, summary, summaries.get(unit));
        Ok(summaries.join_summary(unit, summary))
    }
}

impl HirAnalysisBody {
    fn is_traversable(self) -> bool {
        matches!(self, Self::Block(_) | Self::Expr(_) | Self::HandlerArm(_))
    }
}

fn traverse_body<S, E>(
    interpreter: &mut HirAbstractInterpreter<S>,
    body: HirAnalysisBody,
    state: S::Domain,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<Control<S::Domain>, E>
where
    S: HirAnalysisSemantics,
{
    match body {
        HirAnalysisBody::Block(block) => interpreter.try_block_control(block, state, checkpoint),
        HirAnalysisBody::Expr(expr) => interpreter.try_expr_control(expr, state, checkpoint),
        HirAnalysisBody::HandlerArm(handler) => {
            let body = interpreter
                .context()
                .handler_arm_body(handler)
                .expect("handler arm analysis body should exist in HIR tree");
            interpreter.try_block_control(body, state, checkpoint)
        }
        HirAnalysisBody::External | HirAnalysisBody::Missing => Ok(Control::normal(state)),
    }
}

#[derive(Clone, Debug)]
struct ComponentGraph {
    nodes: Vec<usize>,
    edges: BTreeMap<usize, BTreeSet<usize>>,
}

impl GraphView for ComponentGraph {
    type Node = usize;

    fn nodes(&self) -> Vec<Self::Node> {
        self.nodes.clone()
    }

    fn successors(&self, node: &Self::Node) -> Vec<Self::Node> {
        self.edges
            .get(node)
            .map(|successors| successors.iter().copied().collect())
            .unwrap_or_default()
    }
}

fn ordered_components<U>(graph: &CallGraph<U>) -> Result<Vec<Vec<U>>, ()>
where
    U: AnalysisUnit,
{
    let components = etas_utils::strongly_connected_components(graph)
        .into_iter()
        .map(|component| {
            let mut nodes = component.nodes;
            nodes.sort();
            nodes
        })
        .collect::<Vec<_>>();

    let mut component_by_node = BTreeMap::new();
    for (index, component) in components.iter().enumerate() {
        for node in component {
            component_by_node.insert(*node, index);
        }
    }

    let mut component_graph = ComponentGraph {
        nodes: (0..components.len()).collect(),
        edges: BTreeMap::new(),
    };

    for caller in graph.units() {
        let caller_component = component_by_node[&caller];
        for callee in graph.dependencies(caller) {
            let callee_component = component_by_node[&callee];
            if caller_component != callee_component {
                component_graph
                    .edges
                    .entry(caller_component)
                    .or_default()
                    .insert(callee_component);
            }
        }
    }

    let mut order = topological_sort(&component_graph).map_err(|_| ())?;
    order.reverse();
    Ok(order
        .into_iter()
        .map(|index| components[index].clone())
        .collect())
}

fn push_unique<T>(items: &mut Vec<T>, item: T)
where
    T: PartialEq,
{
    if !items.contains(&item) {
        items.push(item);
    }
}
