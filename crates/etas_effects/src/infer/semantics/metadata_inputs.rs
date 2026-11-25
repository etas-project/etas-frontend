//! Resolve portable application dependencies after their lexical inputs are bound.
use super::{
    engine::EffectSemantics,
    specialize::{EffectSpecializationError as Error, TraceArgumentBinding},
    state::{EffectState, LatentSource},
};
use crate::{
    ParameterCallTarget as Target,
    facts::{
        CallableEnvironment, CallableEnvironmentId, NativeArgumentProjection, NativeCallArgument,
        NativeCallInputs,
    },
};
use etas_code::{CodeFingerprint, descriptor::ValueProjectionStep as Step};
use etas_hir::HirExprId;
use etas_package_metadata::{CallableValueParameter, CallableValueScope, CallableValueSource};
use std::collections::{BTreeMap, BTreeSet};
fn invalid(reason: &str) -> Error {
    Error::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

type InputEquation = (CallableValueParameter, Vec<Step>);
#[derive(Default)]
struct InputSolution {
    active: BTreeSet<InputEquation>,
    expanded: BTreeSet<InputEquation>,
    values: BTreeMap<InputEquation, BTreeSet<LatentSource>>,
    changed: bool,
    nodes: usize,
}

impl EffectSemantics<'_> {
    pub(super) fn metadata_parameter_origins(
        &self,
        key: CodeFingerprint,
        parameter: CallableValueParameter,
        environment: &CallableEnvironment,
        environment_id: Option<CallableEnvironmentId>,
        state: &EffectState,
        call: HirExprId,
    ) -> Result<Option<BTreeSet<LatentSource>>, Error> {
        let mut solution = InputSolution::default();
        loop {
            solution.changed = false;
            solution.expanded.clear();
            let result = self.metadata_parameter_inputs(
                key,
                parameter,
                &[],
                environment,
                environment_id,
                state,
                call,
                &mut solution,
            )?;
            if !solution.changed {
                return Ok(result);
            }
            self.callable_summaries.borrow_mut().invalidate();
        }
    }

    fn metadata_parameter_inputs(
        &self,
        key: CodeFingerprint,
        parameter: CallableValueParameter,
        suffix: &[Step],
        environment: &CallableEnvironment,
        environment_id: Option<CallableEnvironmentId>,
        state: &EffectState,
        call: HirExprId,
        solution: &mut InputSolution,
    ) -> Result<Option<BTreeSet<LatentSource>>, Error> {
        let (graph, _, _, _) = self
            .callable_graph(key)
            .ok_or_else(|| invalid("callback inputs have no checked graph"))?;
        let mut path = graph
            .projection_path(parameter)
            .map_err(|error| invalid(&error.to_string()))?
            .to_vec();
        path.extend_from_slice(suffix);
        let root = Target::Graph {
            graph: key,
            parameter: CallableValueParameter {
                projection: None,
                ..parameter
            },
        };
        if path.is_empty() {
            return Ok(environment.parameters.get(&root).cloned());
        }
        if Step::call_inputs(&path).is_empty() {
            let target = if suffix.is_empty() {
                Target::Graph {
                    graph: key,
                    parameter,
                }
            } else {
                self.intern_external_parameter_projection(root, path.clone())
                    .ok_or_else(|| invalid("callback input path exceeds analysis budget"))?
            };
            if let Some(sources) = environment.parameters.get(&target) {
                return Ok(Some(sources.clone()));
            }
        }
        let equation = (parameter, suffix.to_vec());
        if solution.active.contains(&equation) || !solution.expanded.insert(equation.clone()) {
            return Ok(Some(
                solution.values.get(&equation).cloned().unwrap_or_default(),
            ));
        }
        solution.active.insert(equation.clone());
        if let Some(control) = self.query_control {
            control.checkpoint().map_err(Error::Interrupted)?;
        }
        let result = (|| {
            let original_path = path.clone();
            let mut missing = false;
            Step::map_call_inputs(&mut path, &mut |id| {
                let inputs = graph
                    .call_inputs
                    .get(id as usize)
                    .ok_or_else(|| invalid("callback input reference is out of range"))?;
                let context = CodeFingerprint::of_bytes(
                    "etas.effects.metadata-input-equation.v1",
                    &serde_json::to_vec(&(key, environment_id, id))
                        .expect("checked metadata input identity serializes"),
                );
                let provisional = NativeCallInputs {
                    call,
                    scope: None,
                    context: Some(context),
                    arguments: inputs
                        .arguments
                        .iter()
                        .map(|argument| NativeCallArgument {
                            name: argument.name.clone(),
                            sources: BTreeSet::new(),
                            projections: Vec::new(),
                        })
                        .collect(),
                    joined: true,
                };
                let existing = self
                    .parameter_projection_index
                    .borrow()
                    .joined_inputs
                    .get(&(provisional.scope, provisional.context, call))
                    .copied();
                let native_id = match existing {
                    Some(id) => id,
                    None => self.join_call_inputs(provisional.clone())?,
                };
                let prefix = original_path
                    .iter()
                    .take_while(|step| !Step::call_inputs(std::slice::from_ref(step)).contains(&id))
                    .cloned()
                    .collect::<Vec<_>>();
                let callee = self.metadata_parameter_inputs(
                    key,
                    CallableValueParameter {
                        projection: None,
                        ..parameter
                    },
                    &prefix,
                    environment,
                    environment_id,
                    state,
                    call,
                    solution,
                )?;
                let Some(callee) = callee else {
                    missing = true;
                    return Ok(native_id);
                };
                let paths = self.input_source_paths(&provisional, callee)?;
                let mut arguments = Vec::new();
                for (argument, inferred) in inputs.arguments.iter().zip(paths) {
                    let Some(sources) = self.metadata_input_origins(
                        key,
                        argument.sources.as_deref(),
                        &[],
                        environment,
                        environment_id,
                        state,
                        call,
                        solution,
                    )?
                    else {
                        missing = true;
                        return Ok(native_id);
                    };
                    let mut projections = Vec::new();
                    for projection in &argument.projections {
                        let Some(sources) = self.metadata_input_origins(
                            key,
                            projection.sources.as_deref(),
                            &[],
                            environment,
                            environment_id,
                            state,
                            call,
                            solution,
                        )?
                        else {
                            missing = true;
                            return Ok(native_id);
                        };
                        projections.push(NativeArgumentProjection {
                            path: projection.path.clone(),
                            sources,
                        });
                    }
                    for selected in inferred {
                        if projections
                            .iter()
                            .any(|projection| projection.path == selected)
                        {
                            continue;
                        }
                        let Some(sources) = self.metadata_input_origins(
                            key,
                            argument.sources.as_deref(),
                            &selected,
                            environment,
                            environment_id,
                            state,
                            call,
                            solution,
                        )?
                        else {
                            missing = true;
                            return Ok(native_id);
                        };
                        projections.push(NativeArgumentProjection {
                            path: selected,
                            sources,
                        });
                    }
                    projections.sort_by(|left, right| left.path.cmp(&right.path));
                    arguments.push(NativeCallArgument {
                        name: argument.name.clone(),
                        sources,
                        projections,
                    });
                }
                let incoming = NativeCallInputs {
                    arguments,
                    ..provisional
                };
                let previous = self
                    .native_call_inputs(native_id)
                    .ok_or_else(|| invalid("metadata input equation is missing"))?;
                solution.changed |= previous.as_ref() != &incoming;
                self.join_call_inputs(incoming)
            })?;
            if missing {
                return Ok(None);
            }
            let target = self
                .intern_external_parameter_projection(root, path)
                .ok_or_else(|| invalid("callback input path exceeds analysis budget"))?;
            let bindings = environment
                .parameters
                .iter()
                .map(|(target, origins)| (*target, TraceArgumentBinding::Origins(origins.clone())))
                .collect();
            let Some(binding) =
                self.trace_argument(target, "metadata callback input", &bindings, state)?
            else {
                return Ok(None);
            };
            self.trace_argument_origins_at(&binding, state, call)
                .map(Some)
        })();
        solution.active.remove(&equation);
        let Some(found) = result? else {
            return Ok(None);
        };
        let previous = solution.values.entry(equation).or_default();
        let before = previous.len();
        previous.extend(found);
        let added = previous.len() - before;
        solution.changed |= added > 0;
        solution.nodes += 1 + added;
        if solution.nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid("metadata input equations exceed analysis budget"));
        }
        Ok(Some(previous.clone()))
    }

    fn metadata_input_origins(
        &self,
        key: CodeFingerprint,
        sources: Option<&[CallableValueSource]>,
        suffix: &[Step],
        environment: &CallableEnvironment,
        environment_id: Option<CallableEnvironmentId>,
        state: &EffectState,
        call: HirExprId,
        solution: &mut InputSolution,
    ) -> Result<Option<BTreeSet<LatentSource>>, Error> {
        let Some(sources) = sources else {
            return Ok(Some([LatentSource::Unavailable { expr: call }].into()));
        };
        let mut result = BTreeSet::new();
        for source in sources {
            let source = match *source {
                CallableValueSource::Parameter(parameter) => {
                    let Some(sources) = self.metadata_parameter_inputs(
                        key,
                        parameter,
                        suffix,
                        environment,
                        environment_id,
                        state,
                        call,
                        solution,
                    )?
                    else {
                        return Ok(None);
                    };
                    result.extend(sources);
                    continue;
                }
                CallableValueSource::Existential(object) => {
                    LatentSource::MetadataObject(crate::MetadataObjectSource {
                        graph: key,
                        object,
                        environment: environment_id,
                    })
                }
                CallableValueSource::Value(value) | CallableValueSource::FrozenContract(value) => {
                    LatentSource::Metadata(crate::MetadataCallableSource {
                        graph: key,
                        value,
                        environment: environment_id,
                        analysis: if matches!(source, CallableValueSource::FrozenContract(_)) {
                            crate::CallableAnalysisKind::FrozenContract
                        } else {
                            crate::CallableAnalysisKind::CheckedRevision
                        },
                    })
                }
            };
            let Some(selected) = self.project_value_origins(&[source].into(), suffix, state, call)
            else {
                return Ok(None);
            };
            result.extend(selected);
        }
        Ok(Some(result))
    }

    pub(super) fn metadata_input_environment(
        &self,
        mut source: crate::MetadataCallableSource,
        inputs: u32,
        state: &EffectState,
        call: HirExprId,
    ) -> Result<crate::MetadataCallableSource, Error> {
        let (_, node, _) = self
            .metadata_callable(source)
            .ok_or_else(|| invalid("callback has no checked metadata node"))?;
        let inputs = self
            .native_call_inputs(inputs)
            .ok_or_else(|| invalid("callback inputs are missing"))?;
        if inputs.arguments.len() != node.params.len() {
            return Err(invalid(
                "callback input count differs from its checked signature",
            ));
        }
        let mut environment = match source.environment {
            Some(id) => self.callable_environment(id)?,
            None => CallableEnvironment {
                invocation_origin: None,
                invocation_inputs: Default::default(),
                owner: state
                    .owner
                    .ok_or_else(|| invalid("callback invocation has no checked owner"))?,
                parameters: BTreeMap::new(),
                generics: BTreeMap::new(),
            },
        };
        let positional = inputs
            .arguments
            .iter()
            .filter(|argument| argument.name.is_none())
            .collect::<Vec<_>>();
        let mut position = 0;
        for (index, name) in node.param_names.iter().enumerate() {
            let argument = inputs
                .arguments
                .iter()
                .find(|argument| argument.name.as_ref() == Some(name))
                .or_else(|| {
                    let argument = positional.get(position).copied();
                    position += 1;
                    argument
                })
                .ok_or_else(|| invalid("callback input has no checked parameter"))?;
            let target = Target::Graph {
                graph: source.graph,
                parameter: CallableValueParameter {
                    scope: CallableValueScope::Value(source.value),
                    position: index as u32,
                    projection: None,
                },
            };
            environment
                .parameters
                .insert(target, argument.sources.clone());
            for projection in &argument.projections {
                let target = self
                    .intern_external_parameter_projection(target, projection.path.clone())
                    .ok_or_else(|| invalid("callback input positions exceed analysis budget"))?;
                environment
                    .parameters
                    .insert(target, projection.sources.clone());
            }
        }
        let id = CallableEnvironmentId {
            generic_context: CallableEnvironment::generic_identity(&environment.generics),
            instance: None,
            scope: Some(self.callable_environment_scope(environment.owner)?),
            origin: source.environment.map_or(source.graph, |id| id.origin),
            application: self.callable_environment_site(call)?,
        };
        self.record_callable_environment(id, environment)?;
        source.environment = Some(id);
        Ok(source)
    }
}
