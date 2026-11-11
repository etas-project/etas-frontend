use super::*;

pub(super) fn collect(
    metadata: &ProjectExternalPublicMetadataInput,
) -> Result<Vec<etas_types::ExternalTypeInput>, crate::code::descriptor::ProjectionError> {
    let mut types = Vec::new();
    for summary in &metadata.effect_summaries {
        if let Some(behavior) = summary
            .default_handler_effects
            .as_ref()
            .and_then(|effects| effects.behavior.as_ref())
        {
            collect_parts(&mut types, vec![Part::Behavior(behavior)])?;
        }
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
                crate::ProjectExternalActionTraceInput::Repeat(part)
                | crate::ProjectExternalActionTraceInput::DefaultHandler { body: part, .. } => {
                    traces.push(part)
                }
                _ => {}
            }
        }
        if let Some(requests) = &summary.handler_requests {
            for request in &requests.parameters {
                add_target(&mut types, &request.parameter)?;
            }
            for action in &requests.actions {
                if let Some(signature) = &action.signature {
                    types.extend(
                        signature
                            .params
                            .iter()
                            .chain(std::iter::once(&signature.output))
                            .map(convert_external_type),
                    );
                }
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
            collect_parts(&mut types, vec![Part::Graph(graph)])?;
            for node in &graph.values {
                for ty in node.params.iter().chain(std::iter::once(&node.output)) {
                    add(&mut types, ty)?;
                }
                if let Some(requests) = &node.handler_requests {
                    for action in &requests.actions {
                        if let Some(signature) = &action.signature {
                            for ty in signature
                                .params
                                .iter()
                                .chain(std::iter::once(&signature.output))
                            {
                                add(&mut types, ty)?;
                            }
                        }
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
    for method in &metadata.methods {
        add(&mut types, &method.self_type)?;
        collect_parts(&mut types, vec![Part::Graph(&method.callable)])?;
        for requirement in method
            .requirements
            .trace_requirements
            .iter()
            .chain(&method.requirements.trace_obligations)
        {
            if let etas_package_metadata::MethodTraceRequirement::Clause(clause) = requirement {
                collect_parts(
                    &mut types,
                    [
                        &clause.pattern,
                        &clause.guard,
                        &clause.target,
                        &clause.obligation,
                    ]
                    .into_iter()
                    .flatten()
                    .map(Part::Row)
                    .collect(),
                )?;
            }
        }
    }
    Ok(types)
}

enum Part<'a> {
    Behavior(&'a etas_package_metadata::DefaultBehavior),
    Footprint(&'a etas_package_metadata::EffectFootprint),
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
            Part::Behavior(behavior) => {
                pending.push(Part::Footprint(&behavior.direct));
                for entry in &behavior.guarded {
                    for ty in entry
                        .guards
                        .iter()
                        .flat_map(|guard| &guard.args)
                        .filter_map(|arg| arg.ty.as_ref())
                    {
                        add(types, ty)?;
                    }
                    pending.push(Part::Footprint(&entry.footprint));
                }
            }
            Part::Footprint(footprint) => {
                pending.extend(
                    [
                        &footprint.escaping_effects,
                        &footprint.requested_actions,
                        &footprint.default_actions,
                        &footprint.handled_actions,
                    ]
                    .into_iter()
                    .map(Part::Row),
                );
                if let Some(requests) = &footprint.handler_requests {
                    for action in &requests.actions {
                        for ty in action.action.args.iter().filter_map(|arg| arg.ty.as_ref()) {
                            add(types, ty)?;
                        }
                        if let Some(signature) = &action.signature {
                            for ty in signature
                                .params
                                .iter()
                                .chain(std::iter::once(&signature.output))
                            {
                                add(types, ty)?;
                            }
                        }
                        for binding in action.type_bindings.iter().flatten() {
                            add(types, &binding.ty)?;
                        }
                    }
                    for parameter in &requests.parameters {
                        pending.push(Part::Target(&parameter.parameter));
                        for ty in parameter
                            .excluded
                            .iter()
                            .flat_map(|effect| &effect.args)
                            .filter_map(|arg| arg.ty.as_ref())
                        {
                            add(types, ty)?;
                        }
                    }
                }
                for requirement in footprint
                    .requirements
                    .trace_requirements
                    .iter()
                    .chain(&footprint.requirements.trace_obligations)
                {
                    if let etas_package_metadata::MethodTraceRequirement::Clause(clause) =
                        requirement
                    {
                        pending.extend(
                            [
                                &clause.pattern,
                                &clause.guard,
                                &clause.target,
                                &clause.obligation,
                            ]
                            .into_iter()
                            .flatten()
                            .map(Part::Row),
                        );
                        for bound in &clause.selector_bounds {
                            for ty in &bound.spec_args {
                                add(types, ty)?;
                            }
                        }
                    }
                }
            }
            Part::Target(CallableRequestTarget::Parameter(_)) => {}
            Part::Target(CallableRequestTarget::AgentTools(context)) => add(types, context)?,
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
                for object in &graph.objects {
                    add(types, &object.object_type)?;
                    add(types, &object.source_type)?;
                    for witness in &object.witnesses {
                        for ty in &witness.bound.args {
                            add(types, ty)?;
                        }
                    }
                }
                for node in &graph.values {
                    if let Some(behavior) = &node.default_handler_effects.behavior {
                        pending.push(Part::Behavior(behavior));
                    }
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
                ActionTrace::Repeat(part) | ActionTrace::DefaultHandler { body: part, .. } => {
                    pending.push(Part::Trace(part))
                }
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
