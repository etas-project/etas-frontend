//! Select a callable's returned payload before binding the caller's arguments.
use std::collections::{BTreeMap, BTreeSet};

use etas_code::descriptor::ValueProjectionStep as Step;
use etas_hir::HirExprId;

use super::{
    engine::EffectSemantics,
    specialize::{TraceArgumentBinding, TraceArgumentBindings},
    state::{EffectState, LatentSource},
};
use crate::{CallableReturnSource, EffectUnit, ProjectedReturnSources};

type ProjectionEquation = (LatentSource, Vec<Step>, Option<EffectUnit>, HirExprId);

#[derive(Default)]
pub(super) struct ProjectionEvaluation {
    depth: usize,
    changed: bool,
    nodes: usize,
    active: BTreeSet<ProjectionEquation>,
    expanded: BTreeSet<ProjectionEquation>,
    values: BTreeMap<ProjectionEquation, BTreeSet<LatentSource>>,
}

/// Select the nearest recorded ancestor, or the leaves contained in the
/// requested aggregate. Adjacent payloads never contribute to this selection.
pub(super) fn returned_positions<'a, T>(
    positions: impl IntoIterator<Item = (&'a [Step], &'a T)>,
    path: &[Step],
) -> Vec<(&'a T, Vec<Step>)> {
    let positions = positions.into_iter().collect::<Vec<_>>();
    if let Some((prefix, value)) = positions
        .iter()
        .filter(|(prefix, _)| path.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
    {
        return vec![(*value, path[prefix.len()..].to_vec())];
    }
    let mut selected = Vec::new();
    for (position, value) in positions {
        if position.starts_with(path) {
            selected.push((value, Vec::new()));
        } else if position
            .iter()
            .chain(path)
            .any(|step| matches!(step, Step::Repeat { .. }))
        {
            for suffix in regular_position_remainders(position, path) {
                selected.push((value, suffix));
            }
        }
    }
    selected
}

struct PositionAutomaton {
    edges: Vec<Vec<(Option<Step>, usize)>>,
    // Every state retains an exact expression for its suffix language. This
    // permits a query to continue after selecting a stored generic payload.
    tails: Vec<BTreeSet<Vec<Step>>>,
}

impl PositionAutomaton {
    fn compile(path: &[Step]) -> Self {
        let mut graph = Self {
            edges: vec![Vec::new(), Vec::new()],
            tails: vec![
                BTreeSet::from([path.to_vec()]),
                BTreeSet::from([Vec::new()]),
            ],
        };
        let mut pending = vec![(path, 0usize, 1usize)];
        while let Some((word, start, end)) = pending.pop() {
            let mut from = start;
            for (index, step) in word.iter().enumerate() {
                let to = if index + 1 == word.len() {
                    end
                } else {
                    graph.edges.push(Vec::new());
                    let tails = graph.tails[end]
                        .iter()
                        .map(|tail| {
                            let mut suffix = word[index + 1..].to_vec();
                            suffix.extend_from_slice(tail);
                            suffix
                        })
                        .collect();
                    graph.tails.push(tails);
                    graph.edges.len() - 1
                };
                if let Step::Repeat { alternatives } = step {
                    graph.edges[from].push((None, to));
                    let loop_tails = graph.tails[to]
                        .iter()
                        .map(|tail| {
                            let mut suffix = vec![step.clone()];
                            suffix.extend_from_slice(tail);
                            suffix
                        })
                        .collect::<BTreeSet<_>>();
                    let body_tails = alternatives
                        .iter()
                        .flat_map(|alternative| {
                            loop_tails.iter().map(|tail| {
                                let mut suffix = alternative.clone();
                                suffix.extend_from_slice(tail);
                                suffix
                            })
                        })
                        .collect();
                    let inner_start = graph.edges.len();
                    let inner_end = inner_start + 1;
                    graph.edges.extend([Vec::new(), Vec::new()]);
                    graph.tails.extend([body_tails, loop_tails]);
                    graph.edges[from].push((None, inner_start));
                    graph.edges[inner_end].extend([(None, inner_start), (None, to)]);
                    for alternative in alternatives {
                        pending.push((alternative.as_slice(), inner_start, inner_end));
                    }
                } else {
                    graph.edges[from].push((Some(step.clone()), to));
                }
                from = to;
            }
            if word.is_empty() {
                graph.edges[start].push((None, end));
            }
        }
        graph
    }
}

/// The finite product finds matching prefixes without unrolling either regular
/// selector. A query ending inside a stored position selects that aggregate;
/// a stored position ending first leaves the query's exact suffix language.
fn regular_position_remainders(position: &[Step], path: &[Step]) -> BTreeSet<Vec<Step>> {
    let stored = PositionAutomaton::compile(position);
    let query = PositionAutomaton::compile(path);
    let mut pending = vec![(0usize, 0usize)];
    let mut visited = BTreeSet::new();
    let mut suffixes = BTreeSet::new();
    while let Some((left, right)) = pending.pop() {
        if !visited.insert((left, right)) {
            continue;
        }
        if left == 1 {
            suffixes.extend(query.tails[right].iter().cloned());
        }
        if right == 1 {
            suffixes.insert(Vec::new());
        }
        for (label, to) in &stored.edges[left] {
            if label.is_none() {
                pending.push((*to, right));
            }
        }
        for (label, to) in &query.edges[right] {
            if label.is_none() {
                pending.push((left, *to));
            }
        }
        for (a, to_left) in &stored.edges[left] {
            if a.is_none() {
                continue;
            }
            for (b, to_right) in &query.edges[right] {
                if a == b {
                    pending.push((*to_left, *to_right));
                }
            }
        }
    }
    suffixes
}

impl EffectSemantics<'_> {
    pub(super) fn trace_argument_origins_at(
        &self,
        binding: &TraceArgumentBinding,
        state: &EffectState,
        application: HirExprId,
    ) -> Result<BTreeSet<LatentSource>, super::specialize::EffectSpecializationError> {
        if let TraceArgumentBinding::InputData { argument, path, .. } = binding {
            return self
                .input_argument_sources(argument, path, state, application)
                .ok_or_else(|| {
                    super::specialize::EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "callback argument has no checked selected origins".into(),
                    }
                });
        }
        if let TraceArgumentBinding::ProjectedOrigins { sources, path } = binding {
            return Ok(self
                .project_value_origins(sources, path, state, application)
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: application }].into()));
        }
        self.trace_argument_origins(binding, state)
    }

    pub(super) fn projected_return_bindings(
        &self,
        returned: &BTreeSet<CallableReturnSource>,
        suffix: &[Step],
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        application: HirExprId,
        environment: Option<crate::facts::CallableEnvironmentId>,
    ) -> Option<BTreeSet<LatentSource>> {
        if suffix.is_empty() {
            return self
                .returned_callable_bindings(returned, bindings, state, application, environment)
                .ok()
                .map(|sources| sources.into_iter().collect());
        }
        let mut result = BTreeSet::new();
        for source in returned {
            let target = match source {
                CallableReturnSource::Projection(id) => crate::ParameterCallTarget::Projection(*id),
                CallableReturnSource::Parameter(symbol) => {
                    crate::ParameterCallTarget::Native(*symbol)
                }
                _ => return None,
            };
            let selected = match self
                .trace_argument(target, "returned payload", bindings, state)
                .ok()?
            {
                Some(TraceArgumentBinding::Expression(expr)) => {
                    self.projected_sources(expr, suffix, state)?
                }
                Some(TraceArgumentBinding::ProjectedExpression { expr, mut path }) => {
                    path.extend_from_slice(suffix);
                    self.projected_sources(expr, &path, state)?
                }
                Some(TraceArgumentBinding::Origins(origins)) => {
                    self.project_value_origins(&origins, suffix, state, application)?
                }
                Some(TraceArgumentBinding::ProjectedOrigins { sources, mut path }) => {
                    path.extend_from_slice(suffix);
                    self.project_value_origins(&sources, &path, state, application)?
                }
                Some(TraceArgumentBinding::AgentTools(_)) => return None,
                Some(TraceArgumentBinding::InputData { argument, path, .. }) => {
                    let mut selected = path;
                    selected.extend_from_slice(suffix);
                    self.input_argument_sources(&argument, &selected, state, application)?
                }
                None => {
                    let roots = self
                        .returned_callable_bindings(
                            &BTreeSet::from([source.clone()]),
                            bindings,
                            state,
                            application,
                            environment,
                        )
                        .ok()?;
                    self.project_value_origins(
                        &roots.into_iter().collect(),
                        suffix,
                        state,
                        application,
                    )?
                }
            };
            result.extend(selected);
        }
        Some(result)
    }

    pub(super) fn project_value_origins(
        &self,
        origins: &BTreeSet<LatentSource>,
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
    ) -> Option<BTreeSet<LatentSource>> {
        if path.is_empty() {
            return Some(origins.clone());
        }
        let outer = self.projection_evaluation.borrow().depth == 0;
        self.projection_evaluation.borrow_mut().depth += 1;
        let result = (|| {
            loop {
                if outer {
                    let mut solver = self.projection_evaluation.borrow_mut();
                    solver.changed = false;
                    solver.expanded.clear();
                }
                let mut result = BTreeSet::new();
                for source in origins {
                    let key = (*source, path.to_vec(), state.unit, application);
                    {
                        let mut solver = self.projection_evaluation.borrow_mut();
                        if solver.active.contains(&key) || !solver.expanded.insert(key.clone()) {
                            result.extend(solver.values.get(&key).into_iter().flatten());
                            continue;
                        }
                        solver.active.insert(key.clone());
                    }
                    if let Some(control) = self.query_control {
                        control.checkpoint().ok()?;
                    }
                    let found = self.project_value_origins_step(
                        &[*source].into(),
                        path,
                        state,
                        application,
                    )?;
                    let previous = self
                        .projection_evaluation
                        .borrow()
                        .values
                        .get(&key)
                        .cloned()
                        .unwrap_or_default();
                    let mut joined = previous.clone();
                    joined.extend(found);
                    // Recursive return equations have the same type-preserving
                    // back edges as loop-carried values.
                    self.close_origin_set(&previous, &mut joined);
                    let mut solver = self.projection_evaluation.borrow_mut();
                    solver.active.remove(&key);
                    result.extend(joined.iter().copied());
                    solver.changed |= joined != previous;
                    solver.nodes = solver
                        .nodes
                        .saturating_add(1 + joined.len().saturating_sub(previous.len()));
                    solver.values.insert(key, joined);
                    if solver.nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                        return None;
                    }
                }
                if !outer || !self.projection_evaluation.borrow().changed {
                    return Some(result);
                }
                // A returned payload can refine the exits of a captured callable.
                self.callable_summaries.borrow_mut().invalidate();
            }
        })();
        let mut solver = self.projection_evaluation.borrow_mut();
        solver.depth -= 1;
        if outer {
            *solver = ProjectionEvaluation::default();
        }
        result
    }

    fn project_value_origins_step(
        &self,
        origins: &BTreeSet<LatentSource>,
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
    ) -> Option<BTreeSet<LatentSource>> {
        if let Some(sources) = self.project_parameter_origins(origins.iter().copied(), path) {
            return Some(sources);
        }
        if let [Step::Repeat { alternatives }, rest @ ..] = path {
            let mut pending = origins.iter().copied().collect::<Vec<_>>();
            let mut visited = BTreeSet::new();
            let mut closure = BTreeSet::new();
            while let Some(source) = pending.pop() {
                if !visited.insert(source) {
                    continue;
                }
                if visited.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                    return None;
                }
                if let Some(control) = self.query_control {
                    control.checkpoint().ok()?;
                }
                if let Some(symbolic) = self.project_parameter_origins([source], &path[..1]) {
                    closure.extend(symbolic);
                    continue;
                }
                closure.insert(source);
                for alternative in alternatives {
                    pending.extend(self.project_value_origins(
                        &[source].into(),
                        alternative,
                        state,
                        application,
                    )?);
                }
            }
            return self.project_value_origins(&closure, rest, state, application);
        }
        if let [Step::Invocation { inputs }, rest @ ..] = path {
            let sources = self
                .applied_callable_origins(origins, *inputs, state)
                .ok()?;
            return self.project_value_origins(&sources, rest, state, application);
        }
        let (inputs, rest) = match path {
            [Step::CallResult, rest @ ..] => (None, rest),
            [Step::CallWithArguments { inputs }, rest @ ..] => (Some(*inputs), rest),
            _ => return None,
        };
        self.invoked_value_projection_with_inputs(
            &origins.iter().copied().collect::<Vec<_>>(),
            rest,
            state,
            application,
            inputs,
        )
    }

    pub(super) fn invoked_value_projection(
        &self,
        origins: &[LatentSource],
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
    ) -> Option<BTreeSet<LatentSource>> {
        self.invoked_value_projection_with_inputs(origins, path, state, application, None)
    }

    pub(super) fn invoked_value_projection_with_inputs(
        &self,
        origins: &[LatentSource],
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
        inputs: Option<u32>,
    ) -> Option<BTreeSet<LatentSource>> {
        let mut result = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for source in origins {
            if !seen.insert(*source) {
                continue;
            }
            if matches!(
                source,
                LatentSource::Parameter { .. } | LatentSource::Projection(_)
            ) {
                let mut selected = vec![match inputs {
                    Some(inputs) => Step::CallWithArguments { inputs },
                    None => Step::CallResult,
                }];
                selected.extend_from_slice(path);
                result.extend(self.project_parameter_origins([*source], &selected)?);
                continue;
            }
            if let LatentSource::Metadata(source) = source {
                let source = inputs
                    .map(|inputs| {
                        self.metadata_input_environment(*source, inputs, state, application)
                    })
                    .transpose()
                    .ok()?
                    .unwrap_or(*source);
                result.extend(self.metadata_return_projection(source, path, state, application)?);
                continue;
            }
            let (unit, environment) = match source {
                LatentSource::Unit(unit) => (*unit, None),
                LatentSource::Captured { unit, environment } => (*unit, Some(*environment)),
                LatentSource::Reference {
                    unit, environment, ..
                } => (*unit, *environment),
                _ => return None,
            };
            let parameters = self.invocation_params(unit, environment).ok()??;
            if !parameters.is_empty() && inputs.is_none() {
                return None;
            }
            if !matches!(unit, EffectUnit::Item(_) | EffectUnit::AnonymousFlow { .. }) {
                return None;
            }
            let summary = self
                .latent_source_summary(*source, self.hir.exprs[application].span(&self.hir.blocks))
                .ok()?;
            let Some(exits) = summary.exits.as_ref() else {
                if self.pending_source_return(unit) {
                    // The recursive producer has not transferred yet. This
                    // is a least-fixed-point back edge, not missing evidence.
                    continue;
                }
                return None;
            };
            let mut bindings = environment
                .map(|id| self.native_environment_bindings(id))
                .transpose()
                .ok()?
                .unwrap_or_default();
            if let Some(inputs) = inputs {
                let inputs = self.native_call_inputs(inputs)?;
                if inputs.arguments.len() != parameters.len() {
                    return None;
                }
                let positional = inputs
                    .arguments
                    .iter()
                    .filter(|argument| argument.name.is_none())
                    .collect::<Vec<_>>();
                let mut position = 0;
                for parameter in parameters {
                    let name = &self.hir.symbols.get(parameter)?.name;
                    let argument = inputs
                        .arguments
                        .iter()
                        .find(|argument| argument.name.as_ref() == Some(name))
                        .or_else(|| {
                            let argument = positional.get(position).copied();
                            position += 1;
                            argument
                        })?;
                    bindings.insert(
                        crate::ParameterCallTarget::Native(parameter),
                        TraceArgumentBinding::InputData {
                            argument: std::sync::Arc::new(argument.clone()),
                            path: Vec::new(),
                            call: inputs.call,
                        },
                    );
                }
            }
            let selected = returned_positions(
                exits
                    .value_projections
                    .iter()
                    .map(|(path, sources)| (path.as_slice(), sources)),
                path,
            );
            if selected.is_empty() {
                result.extend(self.projected_return_bindings(
                    &exits.callable_return_sources()?,
                    path,
                    &bindings,
                    state,
                    application,
                    environment,
                )?);
            } else {
                for (sources, suffix) in selected {
                    let ProjectedReturnSources::Known(sources) = sources else {
                        return None;
                    };
                    result.extend(self.projected_return_bindings(
                        sources,
                        &suffix,
                        &bindings,
                        state,
                        application,
                        environment,
                    )?);
                }
            }
        }
        Some(result)
    }
}
