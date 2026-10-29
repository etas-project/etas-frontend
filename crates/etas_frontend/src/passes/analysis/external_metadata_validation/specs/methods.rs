use super::*;
use etas_package_metadata::{ActionTrace, CallableRequestTarget, SpecMethodGenericArgument};

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
    declarations: &BTreeMap<
        Vec<String>,
        (ExternalPackageId, &crate::ProjectExternalSpecSignatureInput),
    >,
) -> Result<(), ExternalEnvironmentValidationError> {
    for metadata in &environment.external_public_metadata {
        let mut pending = Vec::new();
        for implementation in &metadata.spec_impls {
            for method in &implementation.methods {
                graph_targets(&method.callable, &mut pending);
            }
        }
        for summary in &metadata.effect_summaries {
            let mut traces = vec![&summary.action_trace];
            while let Some(trace) = traces.pop() {
                match trace {
                    ProjectExternalActionTraceInput::ParameterCall { parameter } => {
                        pending.push(parameter)
                    }
                    ProjectExternalActionTraceInput::Widened {
                        parameter_calls, ..
                    } => pending.extend(parameter_calls),
                    ProjectExternalActionTraceInput::Seq(parts)
                    | ProjectExternalActionTraceInput::Choice(parts) => traces.extend(parts),
                    ProjectExternalActionTraceInput::Repeat(part) => traces.push(part),
                    _ => {}
                }
            }
            if let Some(requests) = &summary.handler_requests {
                pending.extend(requests.parameters.iter().map(|request| &request.parameter));
            }
            if let Some(graph) = &summary.returned_callables {
                graph_targets(graph, &mut pending);
            }
        }
        while let Some(target) = pending.pop() {
            let CallableRequestTarget::SpecMethod(method) = target else {
                continue;
            };
            let invalid =
                |reason| ExternalEnvironmentValidationError::package(metadata.package, reason);
            let (_, declaration) = declarations.get(&method.spec).ok_or_else(|| {
                invalid(format!(
                    "method evidence references undeclared spec `{}`",
                    method.spec.join(".")
                ))
            })?;
            if declaration.kind != crate::ProjectExternalSpecKindInput::Type
                || declaration.param_names.len() != method.spec_args.len()
            {
                return Err(invalid(
                    "method evidence spec kind or argument arity is inconsistent".into(),
                ));
            }
            let member = declaration
                .methods
                .iter()
                .find(|member| member.name == method.method)
                .ok_or_else(|| {
                    invalid(format!(
                        "method evidence references undeclared member `{}`",
                        method.method
                    ))
                })?;
            let signature = member
                .signature
                .as_ref()
                .ok_or_else(|| invalid("method evidence member has no checked signature".into()))?;
            if signature.params.len() != method.arity as usize {
                return Err(invalid(
                    "method evidence input arity does not match the declaration".into(),
                ));
            }
            let generics = signature
                .generic_params
                .iter()
                .enumerate()
                .filter(|(position, _)| Some(*position as u32) != member.receiver_param)
                .map(|(_, parameter)| parameter)
                .collect::<Vec<_>>();
            if generics.len() != method.generic_arguments.len() {
                return Err(invalid(
                    "method evidence generic argument arity is inconsistent".into(),
                ));
            }
            for (parameter, argument) in generics.iter().zip(&method.generic_arguments) {
                if !matches!(
                    (parameter.kind, argument),
                    (
                        ProjectExternalCallableGenericParamKindInput::Type,
                        SpecMethodGenericArgument::Type(_)
                    ) | (
                        ProjectExternalCallableGenericParamKindInput::Effect,
                        SpecMethodGenericArgument::EffectRow(_)
                    )
                ) {
                    return Err(invalid(
                        "method evidence generic argument kind is inconsistent".into(),
                    ));
                }
            }
            for argument in &method.callable_arguments {
                graph_targets(&argument.sources, &mut pending);
            }
        }
    }
    Ok(())
}

fn graph_targets<'a>(
    graph: &'a etas_package_metadata::CallableValueGraph,
    pending: &mut Vec<&'a CallableRequestTarget>,
) {
    for node in &graph.values {
        if let Some(requests) = &node.handler_requests {
            pending.extend(requests.parameters.iter().map(|request| &request.parameter));
        }
        let mut traces = vec![&node.action_trace];
        while let Some(trace) = traces.pop() {
            match trace {
                ActionTrace::ParameterCall { parameter } => pending.push(parameter),
                ActionTrace::Widened {
                    parameter_calls, ..
                } => pending.extend(parameter_calls),
                ActionTrace::Seq(parts) | ActionTrace::Choice(parts) => traces.extend(parts),
                ActionTrace::Repeat(part) => traces.push(part),
                _ => {}
            }
        }
    }
}
