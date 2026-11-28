//! Input dependencies of a symbolic callback application. Returned payloads
//! must be substituted with these values before they leave the callee scope.
use super::{
    engine::EffectSemantics,
    specialize::EffectSpecializationError as Error,
    state::{EffectState, LatentSource},
};
use crate::facts::{NativeArgumentProjection, NativeCallArgument, NativeCallInputs};
use crate::infer::analysis::dependencies::InputFact;
use crate::{
    ParameterCallTarget as Target,
    facts::{CallableEnvironment, CallableEnvironmentId},
};
use etas_code::{CodeFingerprint, descriptor::ValueProjectionStep as Step};
use etas_hir::{HirArg, HirExpr, HirExprId};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn invalid(reason: &str) -> Error {
    Error::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    /// Bind invocation inputs without selecting the callable's returned value.
    /// The same input table serves effect traces and returned-payload queries.
    pub(super) fn applied_callable_origins(
        &self,
        origins: &BTreeSet<LatentSource>,
        inputs: u32,
        state: &EffectState,
    ) -> Result<BTreeSet<LatentSource>, Error> {
        let recipe = self
            .native_call_inputs(inputs)
            .ok_or_else(|| invalid("invocation inputs are missing"))?;
        let mut result = BTreeSet::new();
        for source in origins {
            // A source lambda still carries lexical captures even when its
            // value has not crossed a return boundary. Bind those creation
            // facts before adding this invocation's own argument slots.
            let lexical;
            let source = if let LatentSource::Unit(unit @ crate::EffectUnit::AnonymousFlow { .. }) =
                source
            {
                // This call invokes the lambda, not its enclosing factory.
                // Its owner's generics remain lexical; the SDK call's own
                // type arguments belong to a different parameter interface.
                lexical = self.capture_native_callable_with_generics(
                    *unit,
                    recipe.call,
                    &Default::default(),
                    state,
                    None,
                )?;
                &lexical
            } else {
                source
            };
            if matches!(
                source,
                LatentSource::Parameter { .. } | LatentSource::Projection(_)
            ) {
                result.extend(
                    self.project_parameter_origins([*source], &[Step::Invocation { inputs }])
                        .ok_or_else(|| invalid("invocation has no checked callable position"))?,
                );
                continue;
            }
            if let LatentSource::Metadata(source) = source {
                result.insert(LatentSource::Metadata(self.metadata_input_environment(
                    *source,
                    inputs,
                    state,
                    recipe.call,
                )?));
                continue;
            }
            let (unit, captured) = match *source {
                LatentSource::Unit(unit) => (unit, None),
                LatentSource::Captured { unit, environment } => (unit, Some(environment)),
                LatentSource::Reference { unit, .. } => {
                    (unit, self.reference_invocation_environment(*source)?)
                }
                LatentSource::FrozenContract {
                    unit,
                    reference,
                    environment,
                } => (
                    unit,
                    match reference {
                        Some(value) => {
                            self.reference_invocation_environment(LatentSource::Reference {
                                unit,
                                value,
                                environment,
                            })?
                        }
                        None => environment,
                    },
                ),
                _ => return Err(invalid("invocation has no checked callable implementation")),
            };
            let parameters = self
                .invocation_params(unit, captured)?
                .ok_or_else(|| invalid("invocation has no checked parameter interface"))?;
            if parameters.len() != recipe.arguments.len() {
                return Err(invalid(&format!(
                    "invocation input arity {} differs from checked signature {} for {unit:?} (recipe {inputs})",
                    recipe.arguments.len(),
                    parameters.len(),
                )));
            }
            let owner = state
                .owner
                .ok_or_else(|| invalid("invocation has no checked owner"))?;
            let mut snapshot = match captured {
                Some(id) => self.callable_environment(id)?,
                None => CallableEnvironment {
                    owner,
                    invocation_origin: None,
                    invocation_inputs: Default::default(),
                    parameters: BTreeMap::new(),
                    generics: self.lexical_generic_values(
                        unit.owner()
                            .ok_or_else(|| invalid("invocation has no declaration owner"))?,
                    )?,
                },
            };
            let origin = match snapshot.invocation_origin {
                Some(origin) => origin,
                None => {
                    let bytes = serde_json::to_vec(source)
                        .map_err(|_| invalid("invocation has no checked creation identity"))?;
                    CodeFingerprint::of_bytes("etas.effects.invocation-creation.v1", &bytes)
                }
            };
            snapshot.invocation_origin = Some(origin);
            snapshot.owner = owner;
            let positional = recipe
                .arguments
                .iter()
                .filter(|argument| argument.name.is_none())
                .collect::<Vec<_>>();
            let mut position = 0;
            for parameter in parameters {
                let name = &self
                    .hir
                    .symbols
                    .get(parameter)
                    .ok_or_else(|| invalid("invocation parameter is missing"))?
                    .name;
                let argument = recipe
                    .arguments
                    .iter()
                    .find(|argument| argument.name.as_ref() == Some(name))
                    .or_else(|| {
                        let argument = positional.get(position).copied();
                        position += 1;
                        argument
                    })
                    .ok_or_else(|| invalid("invocation input has no checked parameter"))?;
                snapshot
                    .parameters
                    .insert(Target::Native(parameter), argument.sources.clone());
                let parameter_owner =
                    match self.hir.symbols.get(parameter).map(|symbol| &symbol.def) {
                        Some(etas_hir::SymbolDef::Param { owner, .. }) => *owner,
                        _ => return Err(invalid("invocation input is not a checked parameter")),
                    };
                for projection in &argument.projections {
                    // An invocation binds positions checked in the callback's
                    // own parameter interface. Caller payload paths can be
                    // more specific than an opaque generic parameter.
                    let id = crate::facts::NativeParameterProjection::path_identity(
                        parameter_owner,
                        parameter,
                        &projection.path,
                    )
                    .map_err(|_| invalid("invocation input has no checked position identity"))?;
                    if self.parameter_projection(id).is_some() {
                        snapshot
                            .parameters
                            .insert(Target::Projection(id), projection.sources.clone());
                    }
                }
            }
            let scope = self.callable_environment_scope(owner)?;
            let id = CallableEnvironmentId {
                generic_context: CallableEnvironment::generic_identity(&snapshot.generics),
                instance: captured.and_then(|id| {
                    if id.scope != Some(scope) {
                        Some(id.lexical_instance())
                    } else {
                        id.instance
                    }
                }),
                scope: Some(scope),
                origin,
                application: self.callable_environment_site(recipe.call)?,
            };
            self.record_invocation_environment(id, recipe.equation_identity(), snapshot)?;
            result.insert(match source {
                LatentSource::FrozenContract { unit, .. } => LatentSource::FrozenContract {
                    unit: *unit,
                    reference: None,
                    environment: Some(id),
                },
                _ => LatentSource::Captured {
                    unit,
                    environment: id,
                },
            });
        }
        Ok(result)
    }

    pub(super) fn native_call_inputs(&self, id: u32) -> Option<Arc<NativeCallInputs>> {
        self.dependencies.read(InputFact::CallInput(id));
        self.parameter_projection_index
            .borrow()
            .call_inputs
            .get(&id)
            .cloned()
    }

    pub(super) fn intern_call_inputs(&self, inputs: NativeCallInputs) -> Result<u32, Error> {
        let bytes = serde_json::to_vec(&inputs)
            .map_err(|_| invalid("call inputs have no checked encoding"))?;
        let key = CodeFingerprint::of_bytes("etas.effects.call-inputs.v1", &bytes);
        let mut index = self.parameter_projection_index.borrow_mut();
        if let Some(id) = index.input_ids.get(&key) {
            self.dependencies.read(InputFact::CallInput(*id));
            return Ok(*id);
        }
        let nodes = inputs.nodes();
        if index.nodes.saturating_add(1 + nodes) > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "callback input graph exceeds the compiler node budget",
            ));
        }
        let id = index
            .call_inputs
            .keys()
            .next_back()
            .map_or(Some(0), |id| id.checked_add(1))
            .ok_or_else(|| invalid("callback input index exceeds u32"))?;
        index.insert(nodes);
        index.input_ids.insert(key, id);
        index.call_inputs.insert(id, Arc::new(inputs));
        self.dependencies.read(InputFact::CallInput(id));
        Ok(id)
    }

    pub(super) fn join_call_inputs(&self, mut inputs: NativeCallInputs) -> Result<u32, Error> {
        let key = (inputs.scope, inputs.context, inputs.call);
        let existing = self
            .parameter_projection_index
            .borrow()
            .joined_inputs
            .get(&key)
            .copied();
        if let Some(id) = existing {
            let original = self
                .native_call_inputs(id)
                .ok_or_else(|| invalid("application input equation is missing"))?;
            // Body equations join control-flow alternatives. A scoped recipe is
            // a derived evaluation in a complete invocation frame, so its next
            // evaluation replaces provisional results from that same frame.
            if inputs.context.is_none() {
                for (incoming, previous) in inputs.arguments.iter_mut().zip(&original.arguments) {
                    incoming.sources.extend(&previous.sources);
                    if inputs.scope.is_some() {
                        self.close_origin_set(&previous.sources, &mut incoming.sources);
                    }
                    for projection in &previous.projections {
                        if let Some(incoming) = incoming
                            .projections
                            .iter_mut()
                            .find(|incoming| incoming.path == projection.path)
                        {
                            incoming.sources.extend(&projection.sources);
                            if inputs.scope.is_some() {
                                self.close_origin_set(&projection.sources, &mut incoming.sources);
                            }
                        } else {
                            incoming.projections.push(projection.clone());
                        }
                    }
                    incoming.projections.sort_by(|a, b| a.path.cmp(&b.path));
                }
            }
            if inputs == *original {
                return Ok(id);
            }
            let mut index = self.parameter_projection_index.borrow_mut();
            let nodes = index
                .nodes
                .saturating_sub(1 + original.nodes())
                .saturating_add(1 + inputs.nodes());
            if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid(
                    "callback input graph exceeds the compiler node budget",
                ));
            }
            index.nodes = nodes;
            let inputs = Arc::new(inputs);
            index.call_inputs.insert(id, inputs.clone());
            let users = index.input_users.get(&id).cloned().unwrap_or_default();
            drop(index);
            let mut projections = self.parameter_projections.borrow_mut();
            let projections = Arc::make_mut(&mut projections);
            for user in users {
                projections
                    .get_mut(&user)
                    .ok_or_else(|| invalid("application input user is missing"))?
                    .call_inputs
                    .insert(id, inputs.clone());
            }
            self.callable_summaries.borrow_mut().invalidate();
            return Ok(id);
        }
        let mut index = self.parameter_projection_index.borrow_mut();
        let nodes = inputs.nodes();
        if index.nodes.saturating_add(1 + nodes) > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "callback input graph exceeds the compiler node budget",
            ));
        }
        let id = index
            .call_inputs
            .keys()
            .next_back()
            .map_or(Some(0), |id| id.checked_add(1))
            .ok_or_else(|| invalid("callback input index exceeds u32"))?;
        index.insert(nodes);
        index.joined_inputs.insert(key, id);
        index.call_inputs.insert(id, Arc::new(inputs));
        self.dependencies.read(InputFact::CallInput(id));
        Ok(id)
    }

    pub(super) fn call_result_step(
        &self,
        call: HirExprId,
        state: &EffectState,
    ) -> Result<Step, Error> {
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(call) else {
            return Err(invalid(
                "symbolic callback application has no checked arguments",
            ));
        };
        let mut arguments = Vec::new();
        for arg in args {
            let (name, expr) = match arg {
                HirArg::Positional(expr) => (None, *expr),
                HirArg::Named { name, value, .. } => (Some(name.clone()), *value),
            };
            let ty = *self
                .types
                .facts
                .expr_types
                .get(&expr)
                .ok_or_else(|| invalid("callback input has no checked type"))?;
            let sources = if self.type_contains_flow(ty) || self.type_has_parameters(ty) {
                self.latent_sources_for_expr(state, expr)
                    .map(|sources| sources.into_iter().collect())
                    .unwrap_or_else(|| [LatentSource::Unavailable { expr }].into())
            } else {
                BTreeSet::new()
            };
            let mut projections = Vec::new();
            for path in self
                .value_payload_paths(ty)
                .ok_or_else(|| invalid("callback input positions exceed analysis budget"))?
            {
                let sources = self
                    .projected_sources(expr, &path, state)
                    .unwrap_or_else(|| [LatentSource::Unavailable { expr }].into());
                projections.push(NativeArgumentProjection { path, sources });
            }
            arguments.push(NativeCallArgument {
                name,
                sources,
                projections,
            });
        }
        Ok(Step::CallWithArguments {
            inputs: self.join_call_inputs(NativeCallInputs {
                call,
                scope: None,
                context: None,
                arguments,
                joined: true,
            })?,
        })
    }

    pub(super) fn input_argument_sources(
        &self,
        argument: &NativeCallArgument,
        path: &[Step],
        state: &EffectState,
        call: HirExprId,
    ) -> Option<BTreeSet<LatentSource>> {
        if path.is_empty() {
            return Some(argument.sources.clone());
        }
        let selected = argument
            .projections
            .iter()
            .filter(|projection| path.starts_with(&projection.path))
            .max_by_key(|projection| projection.path.len());
        if let Some(selected) = selected {
            return self.project_value_origins(
                &selected.sources,
                &path[selected.path.len()..],
                state,
                call,
            );
        }
        self.project_value_origins(&argument.sources, path, state, call)
    }
}
