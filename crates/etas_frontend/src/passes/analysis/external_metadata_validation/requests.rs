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
        validate_target(&parameter.parameter, scope, parameters)?;
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

pub(super) fn validate_target(
    target: &etas_package_metadata::CallableRequestTarget,
    scope: &GenericScope,
    parameters: &[String],
) -> Result<(), String> {
    use etas_package_metadata::{CallableRequestTarget, SpecMethodGenericArgument};
    target
        .validate_resources()
        .map_err(|error| error.to_string())?;
    let method = match target {
        CallableRequestTarget::Parameter(parameter) => {
            return if parameters.contains(parameter) {
                Ok(())
            } else {
                Err(format!(
                    "call evidence parameter `{parameter}` is not declared by the callable"
                ))
            };
        }
        CallableRequestTarget::SpecMethod(method) => method,
    };
    validate_path(&method.spec, "method evidence spec path")?;
    validate_name(&method.method, "method evidence name")?;
    validate_metadata_type(&method.receiver, scope)?;
    for ty in &method.spec_args {
        validate_metadata_type(ty, scope)?;
    }
    for argument in &method.generic_arguments {
        match argument {
            SpecMethodGenericArgument::Type(ty) => validate_metadata_type(ty, scope)?,
            SpecMethodGenericArgument::EffectRow(row) => validate_metadata_row(row, scope)?,
        }
    }
    for argument in &method.callable_arguments {
        etas_package_metadata::validate_callable_value_graph(&argument.sources)
            .map_err(|error| error.to_string())?;
        argument
            .sources
            .validate_references(Some((parameters.len(), &scope.parameter_kinds)))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn validate_metadata_type(
    ty: &etas_package_metadata::Type,
    scope: &GenericScope,
) -> Result<(), String> {
    etas_types::lower::external::package_type_input(ty).map_err(|error| error.message)?;
    let mut pending = vec![ty];
    while let Some(ty) = pending.pop() {
        if ty.kind == etas_package_metadata::TypeKind::Var && !scope.types.contains(&ty.name) {
            return Err(format!(
                "method evidence type variable `{}` is not declared by the callable",
                ty.name
            ));
        }
        pending.extend(&ty.children);
        pending.extend(ty.fields.iter().map(|field| &field.ty));
        for row in ty.effects.iter().chain(&ty.produced_effects) {
            validate_metadata_row(row, scope)?;
        }
    }
    Ok(())
}

fn validate_metadata_row(
    row: &etas_package_metadata::EffectRow,
    scope: &GenericScope,
) -> Result<(), String> {
    etas_types::lower::external::package_effect_row_input(row).map_err(|error| error.message)?;
    if let Some(tail) = &row.tail
        && !scope.effects.contains(tail)
    {
        return Err(format!(
            "method evidence row tail `{tail}` is not declared by the callable"
        ));
    }
    for ty in row
        .effects
        .iter()
        .flat_map(|effect| &effect.args)
        .filter_map(|arg| arg.ty.as_ref())
    {
        validate_metadata_type(ty, scope)?;
    }
    Ok(())
}
