use super::*;
use etas_code::descriptor::ParameterDefaultOwner;
use etas_package_metadata::ParameterDefault;

pub(super) fn validate(
    environment: &ProjectEnvironmentInput,
) -> Result<(), ExternalEnvironmentValidationError> {
    for metadata in &environment.external_public_metadata {
        let fail = |reason| ExternalEnvironmentValidationError::package(metadata.package, reason);
        for default in metadata.parameter_defaults() {
            if let Some(graph) = &default.callable_value {
                etas_package_metadata::validate_callable_value_graph(graph)
                    .map_err(|error| fail(error.to_string()))?;
            }
        }
        for signature in &metadata.flows {
            declaration(
                &signature.param_names,
                signature.params.len(),
                &signature.parameter_defaults,
            )
            .map_err(fail)?;
        }
        for signature in &metadata.agents {
            declaration(
                &signature.param_names,
                signature.input.len(),
                &signature.parameter_defaults,
            )
            .map_err(fail)?;
        }
        for signature in &metadata.tools {
            declaration(
                &signature.param_names,
                signature.input.len(),
                &signature.parameter_defaults,
            )
            .map_err(fail)?;
        }
        for spec in &metadata.spec_signatures {
            if let Some(signature) = &spec.callable {
                declaration(
                    &signature.param_names,
                    signature.params.len(),
                    &signature.parameter_defaults,
                )
                .map_err(fail)?;
            }
            for method in &spec.methods {
                if let Some(signature) = &method.signature {
                    declaration(
                        &signature.param_names,
                        signature.params.len(),
                        &signature.parameter_defaults,
                    )
                    .map_err(fail)?;
                    for default in &signature.parameter_defaults {
                        if !matches!(
                            &default.reference.owner,
                            ParameterDefaultOwner::SpecMethod { .. }
                        ) {
                            return Err(fail(
                                "spec method default has a different declaration kind".into(),
                            ));
                        }
                    }
                }
            }
        }
        for action in &metadata.actions {
            declaration(
                &action.param_names,
                action.params.len(),
                &action.parameter_defaults,
            )
            .map_err(fail)?;
            for default in &action.parameter_defaults {
                if !matches!(
                    &default.reference.owner,
                    ParameterDefaultOwner::Action { .. }
                ) {
                    return Err(fail(
                        "action value default has a different declaration kind".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn declaration(
    names: &[String],
    arity: usize,
    defaults: &[ParameterDefault],
) -> Result<(), String> {
    if (!names.is_empty() || !defaults.is_empty())
        && (names.len() != arity
            || names.iter().any(String::is_empty)
            || names.iter().collect::<BTreeSet<_>>().len() != names.len())
    {
        return Err("value parameter names differ from their declaration signature".into());
    }
    let mut previous = None;
    for default in defaults {
        if default.slot as usize >= arity
            || previous.is_some_and(|previous| previous >= default.slot)
        {
            return Err("declaration defaults have inconsistent calling positions".into());
        }
        previous = Some(default.slot);
        default
            .reference
            .owner
            .validate()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
