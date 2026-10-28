use super::*;

pub(super) fn validate(
    template: &crate::ProjectExternalRequestInstantiationInput,
    scope: &GenericScope,
    parameters: &[String],
) -> Result<(), String> {
    let mut nodes = template
        .actions
        .len()
        .saturating_add(template.parameters.len());
    for action in &template.actions {
        validate_effect_ref(&action.action, scope)?;
        if let Some(bindings) = &action.type_bindings {
            nodes = nodes.saturating_add(bindings.len());
            let mut names = HashSet::new();
            for (name, ty) in bindings {
                validate_name(name, "request generic parameter")?;
                if !names.insert(name) {
                    return Err("request instance has duplicate generic bindings".into());
                }
                validate_type(ty, scope, false)?;
            }
        }
    }
    for parameter in &template.parameters {
        if !parameters.contains(&parameter.parameter) {
            return Err("request template references an undeclared callable parameter".into());
        }
        nodes = nodes.saturating_add(parameter.excluded.len());
        for effect in &parameter.excluded {
            validate_effect_ref(effect, scope)?;
        }
    }
    if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
        return Err("request template exceeds the metadata node budget".into());
    }
    Ok(())
}
