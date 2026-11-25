//! Least fixed point of checked returned-payload equations. Back edges start at
//! the empty payload set; missing callable/effect evidence remains an error.
use super::{
    engine::EffectSemantics,
    specialize::{
        EffectSpecializationError as Error, TraceArgumentBinding as Binding, TraceArgumentBindings,
    },
    state::{EffectState, LatentSource},
};
use crate::{ParameterCallTarget as Target, facts::NativeArgumentProjection};
use etas_code::{CodeFingerprint, descriptor::ValueProjectionStep as Step};
use etas_hir::{HirExpr, HirExprId, ResolveResult, SymbolDef};
use etas_types::{Type, TypeId};
use std::collections::{BTreeMap, BTreeSet};

type Equation = (LatentSource, Vec<Step>, CodeFingerprint);
#[derive(Default)]
pub(super) struct InputSubstitution {
    depth: usize,
    changed: bool,
    nodes: usize,
    active: BTreeSet<Equation>,
    expanded: BTreeSet<Equation>,
    values: BTreeMap<Equation, BTreeSet<LatentSource>>,
}
fn invalid(reason: &str) -> Error {
    Error::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}
fn target(source: LatentSource) -> Option<Target> {
    match source {
        LatentSource::Parameter { symbol, .. } => Some(Target::Native(symbol)),
        LatentSource::Projection(id) => Some(Target::Projection(id)),
        _ => None,
    }
}

impl EffectSemantics<'_> {
    fn input_callee_parameter(&self, mut expr: HirExprId) -> Option<BTreeSet<LatentSource>> {
        let mut path = Vec::new();
        loop {
            match self.hir.exprs.get(expr)? {
                HirExpr::Field { base, field, .. } => {
                    path.insert(0, Step::Field(field.clone()));
                    expr = *base;
                }
                HirExpr::Path(value) => {
                    let (symbol, mut prefix) = match &value.resolution {
                        ResolveResult::Resolved(symbol) => (*symbol, Vec::new()),
                        ResolveResult::PartiallyResolved(partial) => (
                            partial.resolved_prefix?,
                            partial.remaining.iter().cloned().map(Step::Field).collect(),
                        ),
                        _ => return None,
                    };
                    let SymbolDef::Param { owner, .. } = self.hir.symbols.get(symbol)?.def else {
                        return None;
                    };
                    prefix.extend(path);
                    return self.project_parameter_origins(
                        [LatentSource::Parameter { owner, symbol }],
                        &prefix,
                    );
                }
                _ => return None,
            }
        }
    }

    fn input_sources_at(
        &self,
        sources: &BTreeSet<LatentSource>,
        path: &[Step],
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        call: HirExprId,
        context: CodeFingerprint,
    ) -> Result<BTreeSet<LatentSource>, Error> {
        let mut result = BTreeSet::new();
        for source in sources {
            let key = (*source, path.to_vec(), context);
            {
                let mut solver = self.call_input_substitution.borrow_mut();
                if solver.active.contains(&key) || !solver.expanded.insert(key.clone()) {
                    result.extend(solver.values.get(&key).into_iter().flatten());
                    continue;
                }
                solver.active.insert(key.clone());
            }
            if let Some(control) = self.query_control {
                control.checkpoint().map_err(Error::Interrupted)?;
            }
            let found = if let Some(target) = target(*source) {
                let root = self
                    .parameter_projection_target(target)
                    .map_or(target, |(root, _)| root);
                if bindings.contains_key(&target) || bindings.contains_key(&root) {
                    let binding = self
                        .trace_argument(target, "callback input", bindings, state)?
                        .ok_or_else(|| invalid("callback input binding is missing"))?;
                    self.selected_input_binding(binding, path, state, call)?
                } else if path.is_empty() {
                    [*source].into()
                } else {
                    self.project_value_origins(&[*source].into(), path, state, call)
                        .ok_or_else(|| {
                            invalid("callback input selection has no checked type or origins")
                        })?
                }
            } else if let Some(returned) = self.returned_callable_sources(state, &[*source]) {
                let origins = self
                    .returned_callable_bindings(&returned, bindings, state, call, None)?
                    .into_iter()
                    .collect();
                self.project_value_origins(&origins, path, state, call)
                    .ok_or_else(|| invalid("callback input selection has no checked origins"))?
            } else {
                [*source].into()
            };
            let mut solver = self.call_input_substitution.borrow_mut();
            solver.active.remove(&key);
            let previous = solver.values.entry(key).or_default();
            let before = previous.len();
            previous.extend(found);
            let added = previous.len() - before;
            result.extend(previous.iter().copied());
            solver.changed |= added > 0;
            solver.nodes = solver.nodes.saturating_add(1 + added);
            if solver.nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid(
                    "callback input substitution exceeds compiler node budget",
                ));
            }
        }
        Ok(result)
    }

    fn selected_input_binding(
        &self,
        mut binding: Binding,
        suffix: &[Step],
        state: &EffectState,
        call: HirExprId,
    ) -> Result<BTreeSet<LatentSource>, Error> {
        if !suffix.is_empty() {
            binding = match binding {
                Binding::Expression(expr) => Binding::ProjectedExpression {
                    expr,
                    path: suffix.to_vec(),
                },
                Binding::Origins(sources) => Binding::ProjectedOrigins {
                    sources,
                    path: suffix.to_vec(),
                },
                Binding::ProjectedExpression { expr, mut path } => {
                    path.extend_from_slice(suffix);
                    Binding::ProjectedExpression { expr, path }
                }
                Binding::ProjectedOrigins { sources, mut path } => {
                    path.extend_from_slice(suffix);
                    Binding::ProjectedOrigins { sources, path }
                }
                Binding::InputData {
                    argument,
                    mut path,
                    call,
                } => {
                    path.extend_from_slice(suffix);
                    Binding::InputData {
                        argument,
                        path,
                        call,
                    }
                }
                Binding::AgentTools(_) => {
                    return Err(invalid("agent tool input is not a stored payload"));
                }
            };
        }
        self.trace_argument_origins_at(&binding, state, call)
    }

    fn bound_input_paths(
        &self,
        sources: &BTreeSet<LatentSource>,
        bindings: &TraceArgumentBindings,
    ) -> Result<BTreeSet<Vec<Step>>, Error> {
        let mut paths = BTreeSet::new();
        for source in sources {
            let Some(target) = target(*source) else {
                continue;
            };
            match bindings.get(&target) {
                Some(Binding::Expression(expr)) => {
                    if let Some(ty) = self.types.facts.expr_types.get(expr) {
                        paths.extend(self.value_payload_paths(*ty).ok_or_else(|| {
                            invalid("callback input positions exceed analysis budget")
                        })?);
                    }
                }
                Some(Binding::ProjectedExpression { expr, path }) => {
                    if let Some(ty) = self
                        .types
                        .facts
                        .expr_types
                        .get(expr)
                        .and_then(|ty| self.project_value(*ty, path).ok().flatten())
                    {
                        paths.extend(self.value_payload_paths(ty).ok_or_else(|| {
                            invalid("callback input positions exceed analysis budget")
                        })?);
                    }
                }
                Some(Binding::InputData { argument, path, .. }) => {
                    for projection in &argument.projections {
                        if let Some(suffix) = projection.path.strip_prefix(path.as_slice()) {
                            if !suffix.is_empty() {
                                paths.insert(suffix.to_vec());
                            }
                        }
                    }
                }
                Some(Binding::Origins(origins)) => {
                    for origin in origins {
                        let ty = match origin {
                            LatentSource::Parameter { symbol, .. } => {
                                self.symbol_value_type(*symbol)
                            }
                            LatentSource::Projection(id) => self
                                .parameter_projection(*id)
                                .map(|projection| projection.ty),
                            _ => None,
                        };
                        if let Some(ty) = ty {
                            paths.extend(self.value_payload_paths(ty).ok_or_else(|| {
                                invalid("callback input positions exceed analysis budget")
                            })?);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(paths)
    }

    fn input_callee_paths(
        &self,
        inputs: &crate::facts::NativeCallInputs,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        context: CodeFingerprint,
    ) -> Result<Vec<BTreeSet<Vec<Step>>>, Error> {
        let paths = vec![BTreeSet::new(); inputs.arguments.len()];
        let Some(HirExpr::Call { callee, .. }) = self.hir.exprs.get(inputs.call) else {
            return Ok(paths);
        };
        let original = self.input_callee_parameter(*callee).or_else(|| {
            self.latent_sources_for_expr(state, *callee)
                .map(|sources| sources.into_iter().collect())
        });
        let Some(original) = original else {
            return Ok(paths);
        };
        let sources =
            self.input_sources_at(&original, &[], bindings, state, inputs.call, context)?;
        self.input_source_paths(inputs, sources)
    }

    pub(super) fn input_source_paths(
        &self,
        inputs: &crate::facts::NativeCallInputs,
        sources: BTreeSet<LatentSource>,
    ) -> Result<Vec<BTreeSet<Vec<Step>>>, Error> {
        let mut paths = vec![BTreeSet::new(); inputs.arguments.len()];
        for source in sources {
            let (names, mut types, environment, owner) = match source {
                LatentSource::Unit(unit)
                | LatentSource::Captured { unit, .. }
                | LatentSource::Reference { unit, .. } => {
                    let environment = match source {
                        LatentSource::Captured { environment, .. } => Some(environment),
                        LatentSource::Reference { environment, .. } => environment,
                        _ => None,
                    };
                    let Some(parameters) = self.invocation_params(unit, environment)? else {
                        continue;
                    };
                    let names = parameters
                        .iter()
                        .map(|symbol| {
                            self.hir
                                .symbols
                                .get(*symbol)
                                .map(|symbol| symbol.name.clone())
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| invalid("callback has no checked parameter names"))?;
                    let types = parameters
                        .iter()
                        .map(|symbol| self.symbol_value_type(*symbol))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| invalid("callback has no checked parameter types"))?;
                    (Some(names), types, environment, unit.owner())
                }
                LatentSource::Parameter { symbol, .. } => {
                    let Some(Type::Function(function)) = self
                        .symbol_value_type(symbol)
                        .and_then(|ty| self.types.store.get(ty))
                    else {
                        continue;
                    };
                    (None, function.input.clone(), None, None)
                }
                LatentSource::Projection(id) => {
                    let Some(Type::Function(function)) = self
                        .parameter_projection(id)
                        .and_then(|projection| self.types.store.get(projection.ty))
                    else {
                        continue;
                    };
                    (None, function.input.clone(), None, None)
                }
                LatentSource::Metadata(source) => {
                    let (_, node, checked) = self
                        .metadata_callable(source)
                        .ok_or_else(|| invalid("callback has no checked metadata signature"))?;
                    (
                        Some(node.param_names.clone()),
                        checked.params.clone(),
                        None,
                        None,
                    )
                }
                _ => continue,
            };
            if let (Some(environment), Some(owner)) = (environment, owner) {
                let snapshot = self.callable_environment(environment)?;
                let (named, rows) = self.native_environment_generics(owner, &snapshot.generics)?;
                let parameters =
                    self.native_environment_type_parameters(owner, &snapshot.generics)?;
                let named = named.into_iter().collect();
                let rows = self.type_row_arguments(&rows, Some(owner))?;
                let mut substitution = self
                    .type_substitution(&named)
                    .with_type_parameters(&parameters)
                    .with_effect_arguments(&rows);
                types = types
                    .into_iter()
                    .map(|ty| substitution.substitute(ty))
                    .collect::<Result<Vec<TypeId>, _>>()?;
            }
            let mut position = 0;
            for (index, argument) in inputs.arguments.iter().enumerate() {
                let slot = match &argument.name {
                    Some(name) => names
                        .as_ref()
                        .and_then(|names| names.iter().position(|candidate| candidate == name)),
                    None => {
                        let slot = position;
                        position += 1;
                        Some(slot)
                    }
                };
                let Some(ty) = slot.and_then(|slot| types.get(slot)) else {
                    continue;
                };
                paths[index].extend(self.value_payload_paths(*ty).ok_or_else(|| {
                    invalid("callback parameter positions exceed analysis budget")
                })?);
            }
        }
        Ok(paths)
    }

    pub(super) fn bind_projection_inputs(
        &self,
        path: &mut [Step],
        bindings: &TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<(), Error> {
        if Step::call_inputs(path).is_empty() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&bindings.iter().collect::<Vec<_>>())
            .map_err(|_| invalid("callback input bindings have no checked encoding"))?;
        let context = CodeFingerprint::of_bytes("etas.effects.input-substitution.v1", &bytes);
        let outer = self.call_input_substitution.borrow().depth == 0;
        self.call_input_substitution.borrow_mut().depth += 1;
        let original_path = path.to_vec();
        let result = (|| {
            loop {
                if outer {
                    let mut solver = self.call_input_substitution.borrow_mut();
                    solver.changed = false;
                    solver.expanded.clear();
                }
                path.clone_from_slice(&original_path);
                Step::map_call_inputs(path, &mut |id| {
                    let original = self
                        .native_call_inputs(id)
                        .ok_or_else(|| invalid("callback application inputs are missing"))?;
                    let callee_paths =
                        self.input_callee_paths(&original, bindings, state, context)?;
                    let mut inputs = (*original).clone();
                    for ((argument, previous), callee_paths) in inputs
                        .arguments
                        .iter_mut()
                        .zip(&original.arguments)
                        .zip(callee_paths)
                    {
                        argument.sources = self.input_sources_at(
                            &previous.sources,
                            &[],
                            bindings,
                            state,
                            inputs.call,
                            context,
                        )?;
                        for projection in &mut argument.projections {
                            projection.sources = self.input_sources_at(
                                &projection.sources,
                                &[],
                                bindings,
                                state,
                                inputs.call,
                                context,
                            )?;
                        }
                        let mut paths = self.bound_input_paths(&previous.sources, bindings)?;
                        paths.extend(callee_paths);
                        for path in paths {
                            if argument
                                .projections
                                .iter()
                                .any(|projection| projection.path == path)
                            {
                                continue;
                            }
                            let sources = self.input_sources_at(
                                &previous.sources,
                                &path,
                                bindings,
                                state,
                                inputs.call,
                                context,
                            )?;
                            argument
                                .projections
                                .push(NativeArgumentProjection { path, sources });
                        }
                        argument.projections.sort_by(|a, b| a.path.cmp(&b.path));
                    }
                    if inputs == *original {
                        return Ok(id);
                    }
                    if inputs.joined {
                        // The unit identifies the shared callable template;
                        // substituted inputs belong to its current caller.
                        // Joining by template would mix unrelated lexical roots.
                        inputs.scope = state.owner.map(crate::EffectUnit::Item).or(state.unit);
                        inputs.context = state.application;
                        self.join_call_inputs(inputs)
                    } else {
                        self.intern_call_inputs(inputs)
                    }
                })?;
                if !outer || !self.call_input_substitution.borrow().changed {
                    break;
                }
            }
            Ok(())
        })();
        let mut solver = self.call_input_substitution.borrow_mut();
        solver.depth -= 1;
        if outer {
            *solver = InputSubstitution::default();
        }
        result
    }
}
