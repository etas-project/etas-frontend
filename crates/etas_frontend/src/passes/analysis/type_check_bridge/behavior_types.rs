use super::*;

pub(super) fn collect(
    metadata: &ProjectExternalPublicMetadataInput,
) -> Result<Vec<etas_types::ExternalTypeInput>, crate::code::descriptor::ProjectionError> {
    let mut types = Vec::new();
    for summary in &metadata.effect_summaries {
        let mut traces = vec![&summary.action_trace];
        while let Some(trace) = traces.pop() {
            match trace {
                crate::ProjectExternalActionTraceInput::ParameterCall { parameter } => {
                    add_target(&mut types, parameter)?
                }
                crate::ProjectExternalActionTraceInput::Widened {
                    parameter_calls, ..
                } => {
                    for target in parameter_calls {
                        add_target(&mut types, target)?;
                    }
                }
                crate::ProjectExternalActionTraceInput::Seq(parts)
                | crate::ProjectExternalActionTraceInput::Choice(parts) => traces.extend(parts),
                crate::ProjectExternalActionTraceInput::Repeat(part) => traces.push(part),
                _ => {}
            }
        }
        if let Some(requests) = &summary.handler_requests {
            for request in &requests.parameters {
                add_target(&mut types, &request.parameter)?;
            }
            for action in &requests.actions {
                for (_, ty) in action.type_bindings.iter().flatten() {
                    types.push(convert_external_type(ty));
                }
                for arg in &action.action.args {
                    if let ProjectExternalEffectArgInput::Type(ty) = arg {
                        types.push(convert_external_type(ty));
                    }
                }
            }
            for effect in requests
                .parameters
                .iter()
                .flat_map(|parameter| &parameter.excluded)
            {
                for arg in &effect.args {
                    if let ProjectExternalEffectArgInput::Type(ty) = arg {
                        types.push(convert_external_type(ty));
                    }
                }
            }
        }
        if let Some(graph) = &summary.returned_callables {
            for node in &graph.values {
                for ty in node.params.iter().chain(std::iter::once(&node.output)) {
                    add(&mut types, ty)?;
                }
                if let Some(requests) = &node.handler_requests {
                    for action in &requests.actions {
                        for binding in action.type_bindings.iter().flatten() {
                            add(&mut types, &binding.ty)?;
                        }
                        for arg in &action.action.args {
                            if let Some(ty) = &arg.ty {
                                add(&mut types, ty)?;
                            }
                        }
                    }
                    for effect in requests
                        .parameters
                        .iter()
                        .flat_map(|parameter| &parameter.excluded)
                    {
                        for arg in &effect.args {
                            if let Some(ty) = &arg.ty {
                                add(&mut types, ty)?;
                            }
                        }
                    }
                }
            }
        }
    }
    for implementation in &metadata.spec_impls {
        for method in &implementation.methods {
            collect_parts(&mut types, vec![Part::Graph(&method.callable)])?;
        }
    }
    Ok(types)
}

enum Part<'a> {
    Target(&'a etas_package_metadata::CallableRequestTarget),
    Graph(&'a etas_package_metadata::CallableValueGraph),
    Trace(&'a etas_package_metadata::ActionTrace),
    Row(&'a etas_package_metadata::EffectRow),
}

fn add_target(
    types: &mut Vec<etas_types::ExternalTypeInput>,
    target: &etas_package_metadata::CallableRequestTarget,
) -> Result<(), crate::code::descriptor::ProjectionError> {
    collect_parts(types, vec![Part::Target(target)])
}

fn collect_parts(
    types: &mut Vec<etas_types::ExternalTypeInput>,
    mut pending: Vec<Part<'_>>,
) -> Result<(), crate::code::descriptor::ProjectionError> {
    use etas_package_metadata::{ActionTrace, CallableRequestTarget, SpecMethodGenericArgument};
    while let Some(part) = pending.pop() {
        match part {
            Part::Target(CallableRequestTarget::Parameter(_)) => {}
            Part::Target(CallableRequestTarget::SpecMethod(method)) => {
                add(types, &method.receiver)?;
                for ty in &method.spec_args {
                    add(types, ty)?;
                }
                for argument in &method.generic_arguments {
                    match argument {
                        SpecMethodGenericArgument::Type(ty) => add(types, ty)?,
                        SpecMethodGenericArgument::EffectRow(row) => pending.push(Part::Row(row)),
                    }
                }
                pending.extend(
                    method
                        .callable_arguments
                        .iter()
                        .map(|argument| Part::Graph(&argument.sources)),
                );
            }
            Part::Graph(graph) => {
                for node in &graph.values {
                    for ty in node.params.iter().chain(std::iter::once(&node.output)) {
                        add(types, ty)?;
                    }
                    pending.extend(
                        [
                            &node.public_effects,
                            &node.requested_actions,
                            &node.handled_requested_actions,
                            &node.default_handler_effects.direct_effects,
                            &node.default_handler_effects.default_actions,
                        ]
                        .into_iter()
                        .map(Part::Row),
                    );
                    pending.push(Part::Trace(&node.action_trace));
                    if let Some(requests) = &node.handler_requests {
                        pending.extend(
                            requests
                                .parameters
                                .iter()
                                .map(|parameter| Part::Target(&parameter.parameter)),
                        );
                        for action in &requests.actions {
                            for binding in action.type_bindings.iter().flatten() {
                                add(types, &binding.ty)?;
                            }
                            for ty in action.action.args.iter().filter_map(|arg| arg.ty.as_ref()) {
                                add(types, ty)?;
                            }
                        }
                    }
                }
            }
            Part::Trace(trace) => match trace {
                ActionTrace::ParameterCall { parameter } => pending.push(Part::Target(parameter)),
                ActionTrace::Widened {
                    parameter_calls, ..
                } => pending.extend(parameter_calls.iter().map(Part::Target)),
                ActionTrace::Seq(parts) | ActionTrace::Choice(parts) => {
                    pending.extend(parts.iter().map(Part::Trace))
                }
                ActionTrace::Repeat(part) => pending.push(Part::Trace(part)),
                _ => {}
            },
            Part::Row(row) => {
                for ty in row
                    .effects
                    .iter()
                    .flat_map(|effect| &effect.args)
                    .filter_map(|arg| arg.ty.as_ref())
                {
                    add(types, ty)?;
                }
            }
        }
    }
    Ok(())
}

fn add(
    types: &mut Vec<etas_types::ExternalTypeInput>,
    ty: &etas_package_metadata::Type,
) -> Result<(), crate::code::descriptor::ProjectionError> {
    types.push(
        etas_types::lower::external::package_type_input(ty)
            .map_err(|error| crate::code::descriptor::ProjectionError::Invalid(error.message))?,
    );
    Ok(())
}
