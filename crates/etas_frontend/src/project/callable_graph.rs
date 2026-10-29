use etas_package_metadata as metadata;
use etas_types::TypeOutput;
use etas_types::lower::external::{
    CheckedExternalTypeResolver, package_effect_arg_input, package_effect_row_input,
    package_type_input,
};

pub(crate) fn lower_callable_graph(
    graph: &metadata::CallableValueGraph,
    types: &TypeOutput,
) -> Result<Vec<etas_effects::ExternalCallableValueMetadata>, String> {
    let resolver = CheckedExternalTypeResolver::new(types);
    lower_graph(graph, &resolver)
}

pub(crate) fn lower_callable_request_target(
    target: &metadata::CallableRequestTarget,
    types: &TypeOutput,
) -> Result<etas_effects::ExternalCallableRequestTarget, String> {
    request_target(target, &CheckedExternalTypeResolver::new(types))
}

fn request_target(
    target: &metadata::CallableRequestTarget,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<etas_effects::ExternalCallableRequestTarget, String> {
    use etas_effects::{
        ExternalCallableRequestTarget as Target, ExternalSpecMethodGenericArgument as Argument,
    };
    Ok(match target {
        metadata::CallableRequestTarget::Parameter(parameter) => {
            Target::Parameter(parameter.clone())
        }
        metadata::CallableRequestTarget::SpecMethod(method) => Target::SpecMethod(
            std::sync::Arc::new(etas_effects::ExternalSpecMethodInvocation {
                identity: target.fingerprint().map_err(|error| error.to_string())?,
                receiver: type_id(&method.receiver, resolver)?,
                spec: resolver
                    .spec_identity(&method.spec)
                    .map_err(|error| error.message)?,
                spec_args: method
                    .spec_args
                    .iter()
                    .map(|ty| type_id(ty, resolver))
                    .collect::<Result<_, _>>()?,
                method: method.method.clone(),
                arity: method.arity,
                generic_arguments: method
                    .generic_arguments
                    .iter()
                    .map(|argument| {
                        Ok(match argument {
                            metadata::SpecMethodGenericArgument::Type(ty) => {
                                Argument::Type(type_id(ty, resolver)?)
                            }
                            metadata::SpecMethodGenericArgument::EffectRow(row) => {
                                let projected =
                                    package_effect_row_input(row).map_err(|error| error.message)?;
                                Argument::EffectRow(
                                    resolver
                                        .effect_row(&projected)
                                        .map_err(|error| error.message)?,
                                )
                            }
                        })
                    })
                    .collect::<Result<_, String>>()?,
                callable_arguments: method
                    .callable_arguments
                    .iter()
                    .map(|argument| {
                        Ok(etas_effects::ExternalSpecMethodCallableArgument {
                            position: argument.position,
                            sources: argument.sources.clone(),
                            values: lower_graph(&argument.sources, resolver)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            }),
        ),
    })
}

pub(crate) fn lower_callable_request_target_in_scope(
    target: &metadata::CallableRequestTarget,
    types: &TypeOutput,
    parameters: &[etas_types::CallableGenericParam],
) -> Result<etas_effects::ExternalCallableRequestTarget, String> {
    let resolver = CheckedExternalTypeResolver::for_callable(types, parameters)
        .map_err(|error| error.message)?;
    request_target(target, &resolver)
}

pub(crate) fn lower_callable_graph_in_scope(
    graph: &metadata::CallableValueGraph,
    types: &TypeOutput,
    parameters: &[etas_types::CallableGenericParam],
) -> Result<Vec<etas_effects::ExternalCallableValueMetadata>, String> {
    let resolver = CheckedExternalTypeResolver::for_callable(types, parameters)
        .map_err(|error| error.message)?;
    lower_graph(graph, &resolver)
}

fn lower_graph(
    graph: &metadata::CallableValueGraph,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<Vec<etas_effects::ExternalCallableValueMetadata>, String> {
    graph
        .values
        .iter()
        .map(|node| {
            Ok(etas_effects::ExternalCallableValueMetadata {
                handler_requests: node
                    .handler_requests
                    .as_ref()
                    .map(|requests| request_template(requests, resolver))
                    .transpose()?,
                params: node
                    .params
                    .iter()
                    .map(|ty| type_id(ty, resolver))
                    .collect::<Result<_, _>>()?,
                output: type_id(&node.output, resolver)?,
                public_effects: row(&node.public_effects, resolver)?,
                requested_actions: row(&node.requested_actions, resolver)?,
                handled_requested_actions: row(&node.handled_requested_actions, resolver)?,
                default_handler_effects: etas_effects::ExternalDefaultHandlerEffectsMetadata {
                    direct_effects: row(&node.default_handler_effects.direct_effects, resolver)?,
                    default_actions: row(&node.default_handler_effects.default_actions, resolver)?,
                    handlers: node
                        .default_handler_effects
                        .handlers
                        .iter()
                        .map(|handler| {
                            Ok(etas_effects::ExternalDefaultHandlerContributionMetadata {
                                action: effect(&handler.action, resolver)?,
                                produced: row(&handler.produced, resolver)?,
                            })
                        })
                        .collect::<Result<_, String>>()?,
                },
                action_trace: trace(&node.action_trace, resolver)?,
            })
        })
        .collect()
}

fn request_template(
    template: &metadata::RequestInstantiationTemplate,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<etas_effects::ExternalRequestInstantiationMetadata, String> {
    Ok(etas_effects::ExternalRequestInstantiationMetadata {
        actions: template
            .actions
            .iter()
            .map(|instance| {
                Ok(etas_effects::ExternalActionRequestMetadata {
                    action: effect(&instance.action, resolver)?,
                    type_bindings: instance
                        .type_bindings
                        .as_ref()
                        .map(|bindings| {
                            bindings
                                .iter()
                                .map(|binding| {
                                    Ok((binding.parameter.clone(), type_id(&binding.ty, resolver)?))
                                })
                                .collect::<Result<_, String>>()
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<_, String>>()?,
        parameters: template
            .parameters
            .iter()
            .map(|parameter| {
                Ok(etas_effects::ExternalParameterRequestMetadata {
                    parameter: request_target(&parameter.parameter, resolver)?,
                    excluded: parameter
                        .excluded
                        .iter()
                        .map(|value| effect(value, resolver))
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, String>>()?,
    })
}

fn type_id(
    ty: &metadata::Type,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<etas_types::TypeId, String> {
    let projected = package_type_input(ty).map_err(|error| error.message)?;
    resolver.type_id(&projected).map_err(|error| error.message)
}

fn row(
    row: &metadata::EffectRow,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<etas_effects::ExternalEffectRowMetadata, String> {
    let row = package_effect_row_input(row).map_err(|error| error.message)?;
    Ok(etas_effects::ExternalEffectRowMetadata {
        effects: row
            .effects
            .into_iter()
            .map(|effect| {
                Ok::<_, String>(etas_effects::ExternalEffectMetadata {
                    path: effect.path,
                    args: effect
                        .args
                        .iter()
                        .map(|arg| resolver.effect_arg(arg).map_err(|error| error.message))
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect::<Result<_, _>>()?,
        tail: row.tail,
    })
}

fn effect(
    value: &metadata::EffectRef,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<etas_effects::ExternalEffectMetadata, String> {
    Ok(etas_effects::ExternalEffectMetadata {
        path: value.path.clone(),
        args: value
            .args
            .iter()
            .map(|arg| {
                let projected = package_effect_arg_input(arg).map_err(|error| error.message)?;
                resolver
                    .effect_arg(&projected)
                    .map_err(|error| error.message)
            })
            .collect::<Result<_, String>>()?,
    })
}

fn trace(
    value: &metadata::ActionTrace,
    resolver: &CheckedExternalTypeResolver<'_>,
) -> Result<etas_effects::ExternalActionTraceMetadata, String> {
    use etas_effects::ExternalActionTraceMetadata as T;
    Ok(match value {
        metadata::ActionTrace::Empty => T::Empty,
        metadata::ActionTrace::Event(event) => T::Event {
            action: effect(&event.action, resolver)?,
            source: match event.source {
                metadata::ActionTraceEventSource::Perform => {
                    etas_effects::ActionEventSource::Perform
                }
                metadata::ActionTraceEventSource::StdIntrinsic => {
                    etas_effects::ActionEventSource::StdIntrinsic
                }
                metadata::ActionTraceEventSource::AgentCall => {
                    etas_effects::ActionEventSource::AgentCall
                }
                metadata::ActionTraceEventSource::ExternalMetadata => {
                    etas_effects::ActionEventSource::ExternalMetadata
                }
                metadata::ActionTraceEventSource::Transfer => {
                    etas_effects::ActionEventSource::Transfer
                }
                metadata::ActionTraceEventSource::Unknown => {
                    etas_effects::ActionEventSource::Unknown
                }
            },
        },
        metadata::ActionTrace::ParameterCall { parameter } => T::ParameterCall {
            parameter: request_target(parameter, resolver)?,
        },
        metadata::ActionTrace::Seq(parts) => T::Seq(
            parts
                .iter()
                .map(|part| trace(part, resolver))
                .collect::<Result<_, _>>()?,
        ),
        metadata::ActionTrace::Choice(parts) => T::Choice(
            parts
                .iter()
                .map(|part| trace(part, resolver))
                .collect::<Result<_, _>>()?,
        ),
        metadata::ActionTrace::Repeat(part) => T::Repeat(Box::new(trace(part, resolver)?)),
        metadata::ActionTrace::UnknownOrder(actions) => T::UnknownOrder(
            actions
                .iter()
                .map(|value| effect(value, resolver))
                .collect::<Result<_, _>>()?,
        ),
        metadata::ActionTrace::Widened {
            actions,
            parameter_calls,
        } => T::Widened {
            actions: actions
                .iter()
                .map(|value| effect(value, resolver))
                .collect::<Result<_, _>>()?,
            parameter_calls: parameter_calls
                .iter()
                .map(|target| request_target(target, resolver))
                .collect::<Result<_, _>>()?,
        },
    })
}
