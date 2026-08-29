use std::collections::{HashMap, HashSet};

use crate::{
    ProjectExternalActionTraceInput, ProjectExternalCallableGenericParamInput,
    ProjectExternalCallableGenericParamKindInput, ProjectExternalEffectArgInput,
    ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalPublicMetadataInput, ProjectExternalTypeInput,
};

#[derive(Clone, Debug, Default)]
struct GenericScope {
    types: HashSet<String>,
    effects: HashSet<String>,
}

pub(super) fn validate_external_metadata(
    metadata: &ProjectExternalPublicMetadataInput,
) -> Result<(), String> {
    let mut callable_scopes = HashMap::<Vec<String>, GenericScope>::new();
    for item in metadata
        .types
        .iter()
        .chain(&metadata.values)
        .chain(&metadata.enums)
        .chain(&metadata.effects)
        .chain(&metadata.trace_specs)
    {
        validate_path(&item.path, "export path")?;
        if let Some(ty) = &item.ty {
            validate_type(ty, &GenericScope::default(), true)?;
        }
    }
    for flow in &metadata.flows {
        let scope = validate_callable(
            &flow.path,
            &flow.generic_params,
            &flow.params,
            &flow.output,
            flow.effects.as_ref(),
        )?;
        callable_scopes.insert(flow.path.clone(), scope);
    }
    for agent in &metadata.agents {
        let scope = validate_callable(
            &agent.path,
            &agent.generic_params,
            &agent.input,
            &agent.output,
            agent.effects.as_ref(),
        )?;
        callable_scopes.insert(agent.path.clone(), scope);
    }
    for tool in &metadata.tools {
        let scope = validate_callable(
            &tool.path,
            &tool.generic_params,
            &tool.input,
            &tool.output,
            tool.effects.as_ref(),
        )?;
        callable_scopes.insert(tool.path.clone(), scope);
    }
    for schema in &metadata.tool_schemas {
        validate_path(&schema.path, "tool schema path")?;
        serde_json::from_str::<serde_json::Value>(&schema.schema_json)
            .map_err(|error| format!("tool schema is not valid JSON: {error}"))?;
    }
    for action in &metadata.actions {
        let scope = validate_callable(
            &action.path,
            &action.generic_params,
            &action.params,
            &action.output,
            None,
        )?;
        if action.effect_args.len() != action.selector_param_names.len()
            || action.effect_args.len() != action.selector_defaults.len()
        {
            return Err(format!(
                "action `{}` selector_defaults length must match effect_args and selector_param_names",
                action.path.join("."),
            ));
        }
        for default in action.selector_defaults.iter().flatten() {
            validate_effect_arg(default, &scope)?;
        }
    }
    for value in &metadata.values {
        callable_scopes.entry(value.path.clone()).or_default();
    }
    validate_spec_facts(metadata)?;
    validate_effect_summaries(metadata, &callable_scopes)?;
    for summary in &metadata.action_summaries {
        validate_path(&summary.action, "action summary path")?;
        if summary.args.iter().any(String::is_empty) {
            return Err(format!(
                "action summary `{}` contains an empty selector",
                summary.action.join(".")
            ));
        }
    }
    for summary in &metadata.trace_spec_summaries {
        validate_path(&summary.trace_spec, "trace spec summary path")?;
        let scope = metadata
            .spec_signatures
            .iter()
            .find(|signature| signature.path == summary.trace_spec)
            .map(|signature| GenericScope {
                types: signature.param_names.iter().cloned().collect(),
                effects: HashSet::new(),
            })
            .unwrap_or_default();
        for clause in &summary.clauses {
            for row in [
                clause.pattern.as_ref(),
                clause.guard.as_ref(),
                clause.target.as_ref(),
                clause.obligation.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                validate_effect_row(row, &scope)?;
            }
        }
    }
    for re_export in &metadata.re_exports {
        validate_path(&re_export.from, "re-export source path")?;
        validate_path(&re_export.exported, "re-export target path")?;
    }
    Ok(())
}

fn validate_spec_facts(metadata: &ProjectExternalPublicMetadataInput) -> Result<(), String> {
    for signature in &metadata.spec_signatures {
        validate_path(&signature.path, "spec path")?;
        let scope = GenericScope {
            types: signature.param_names.iter().cloned().collect(),
            effects: HashSet::new(),
        };
        for bound in &signature.super_specs {
            validate_path(&bound.spec, "super spec path")?;
            validate_types(&bound.args, &scope, false)?;
        }
        if let Some(callable) = &signature.callable {
            validate_callable(
                &callable.path,
                &callable.generic_params,
                &callable.params,
                &callable.output,
                callable.effects.as_ref(),
            )?;
        }
        for method in &signature.methods {
            validate_path(&method.path, "spec method path")?;
            if method.name.is_empty() {
                return Err("spec method name must not be empty".to_owned());
            }
            if let Some(callable) = &method.signature {
                validate_callable(
                    &callable.path,
                    &callable.generic_params,
                    &callable.params,
                    &callable.output,
                    callable.effects.as_ref(),
                )?;
            }
        }
    }
    for implementation in &metadata.spec_impls {
        validate_path(&implementation.spec, "spec implementation path")?;
        validate_type(&implementation.self_type, &GenericScope::default(), true)?;
        validate_types(&implementation.args, &GenericScope::default(), true)?;
        if implementation.methods.iter().any(String::is_empty) {
            return Err("spec implementation contains an empty method name".to_owned());
        }
    }
    for satisfaction in &metadata.type_spec_satisfactions {
        validate_path(&satisfaction.spec, "type satisfaction spec path")?;
        validate_type(&satisfaction.self_type, &GenericScope::default(), true)?;
        validate_types(&satisfaction.args, &GenericScope::default(), true)?;
    }
    for satisfaction in &metadata.callable_spec_satisfactions {
        validate_path(&satisfaction.item, "callable satisfaction item path")?;
        validate_path(&satisfaction.spec, "callable satisfaction spec path")?;
        validate_types(&satisfaction.args, &GenericScope::default(), true)?;
    }
    for conformance in &metadata.trace_spec_conformances {
        validate_path(&conformance.item, "trace conformance item path")?;
        if let crate::ProjectExternalTraceSpecConformanceTargetInput::Named { spec, args } =
            &conformance.target
        {
            validate_path(spec, "trace conformance spec path")?;
            validate_types(args, &GenericScope::default(), true)?;
        }
    }
    Ok(())
}

fn validate_effect_summaries(
    metadata: &ProjectExternalPublicMetadataInput,
    callable_scopes: &HashMap<Vec<String>, GenericScope>,
) -> Result<(), String> {
    for summary in &metadata.effect_summaries {
        validate_path(&summary.item, "effect summary item path")?;
        let Some(scope) = callable_scopes.get(&summary.item) else {
            return Err(format!(
                "effect summary `{}` has no matching callable or exported value signature",
                summary.item.join(".")
            ));
        };
        validate_effect_row(&summary.public_effects, scope)?;
        validate_effect_row(&summary.requested_actions, scope)?;
        validate_effect_row(&summary.handled_requested_actions, scope)?;
        for latent in &summary.latent_flows {
            validate_effect_row(&latent.declared_bound, scope)?;
            validate_effect_row(&latent.inferred_effects, scope)?;
        }
        let parameter_names = metadata
            .flows
            .iter()
            .find(|callable| callable.path == summary.item)
            .map(|callable| callable.param_names.as_slice())
            .or_else(|| {
                metadata
                    .agents
                    .iter()
                    .find(|callable| callable.path == summary.item)
                    .map(|callable| callable.param_names.as_slice())
            })
            .or_else(|| {
                metadata
                    .tools
                    .iter()
                    .find(|callable| callable.path == summary.item)
                    .map(|callable| callable.param_names.as_slice())
            })
            .unwrap_or_default();
        validate_action_trace(&summary.action_trace, scope, parameter_names)?;
    }
    Ok(())
}

fn validate_action_trace(
    trace: &ProjectExternalActionTraceInput,
    scope: &GenericScope,
    parameter_names: &[String],
) -> Result<(), String> {
    match trace {
        ProjectExternalActionTraceInput::Empty => Ok(()),
        ProjectExternalActionTraceInput::Event { action, .. } => validate_effect_ref(action, scope),
        ProjectExternalActionTraceInput::ParameterCall { parameter } => {
            if parameter_names.iter().any(|name| name == parameter) {
                Ok(())
            } else {
                Err(format!(
                    "action trace parameter `{parameter}` is not declared by the callable"
                ))
            }
        }
        ProjectExternalActionTraceInput::Seq(children)
        | ProjectExternalActionTraceInput::Choice(children) => {
            for child in children {
                validate_action_trace(child, scope, parameter_names)?;
            }
            Ok(())
        }
        ProjectExternalActionTraceInput::Repeat(child) => {
            validate_action_trace(child, scope, parameter_names)
        }
        ProjectExternalActionTraceInput::UnknownOrder(actions) => {
            for action in actions {
                validate_effect_ref(action, scope)?;
            }
            Ok(())
        }
    }
}

fn validate_callable(
    path: &[String],
    params: &[ProjectExternalCallableGenericParamInput],
    inputs: &[ProjectExternalTypeInput],
    output: &ProjectExternalTypeInput,
    effects: Option<&ProjectExternalEffectRowInput>,
) -> Result<GenericScope, String> {
    validate_path(path, "callable path")?;
    let scope = generic_scope(path, params)?;
    for param in params {
        for bound in &param.bounds {
            validate_path(&bound.spec, "generic bound spec path")?;
            validate_types(&bound.args, &scope, false)?;
        }
    }
    validate_types(inputs, &scope, false)?;
    validate_type(output, &scope, false)?;
    if let Some(row) = effects {
        validate_effect_row(row, &scope)?;
    }
    Ok(scope)
}

fn generic_scope(
    path: &[String],
    params: &[ProjectExternalCallableGenericParamInput],
) -> Result<GenericScope, String> {
    let mut names = HashSet::new();
    let mut scope = GenericScope::default();
    for param in params {
        if param.name.is_empty() || !names.insert(param.name.clone()) {
            return Err(format!(
                "callable `{}` has an empty or duplicate generic parameter",
                path.join(".")
            ));
        }
        match param.kind {
            ProjectExternalCallableGenericParamKindInput::Type => {
                scope.types.insert(param.name.clone());
            }
            ProjectExternalCallableGenericParamKindInput::Effect => {
                scope.effects.insert(param.name.clone());
            }
        }
    }
    Ok(scope)
}

fn validate_type(
    ty: &ProjectExternalTypeInput,
    scope: &GenericScope,
    allow_unbound_vars: bool,
) -> Result<(), String> {
    match ty {
        ProjectExternalTypeInput::Primitive(name) => validate_name(name, "primitive name"),
        ProjectExternalTypeInput::Var(name) => {
            validate_name(name, "type variable")?;
            if !allow_unbound_vars && !scope.types.contains(name) {
                return Err(format!(
                    "type variable `{name}` is not declared by the callable"
                ));
            }
            Ok(())
        }
        ProjectExternalTypeInput::Named(path) => validate_path(path, "named type path"),
        ProjectExternalTypeInput::Applied { path, args } => {
            validate_path(path, "applied type path")?;
            validate_types(args, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Alias { path, target } => {
            validate_path(path, "alias path")?;
            validate_type(target, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Nominal {
            path,
            representation,
        } => {
            validate_path(path, "nominal type path")?;
            if let Some(representation) = representation {
                validate_type(representation, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Array(inner)
        | ProjectExternalTypeInput::List(inner)
        | ProjectExternalTypeInput::Set(inner)
        | ProjectExternalTypeInput::Range(inner)
        | ProjectExternalTypeInput::Slice(inner)
        | ProjectExternalTypeInput::Option(inner)
        | ProjectExternalTypeInput::Message(inner)
        | ProjectExternalTypeInput::MemorySelection(inner)
        | ProjectExternalTypeInput::MemoryRegion(inner) => {
            validate_type(inner, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Map { key, value }
        | ProjectExternalTypeInput::Store { key, value }
        | ProjectExternalTypeInput::Result {
            ok: key,
            err: value,
        } => {
            validate_type(key, scope, allow_unbound_vars)?;
            validate_type(value, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Record { fields } => {
            let mut names = HashSet::new();
            for field in fields {
                if field.name.is_empty() || !names.insert(field.name.as_str()) {
                    return Err("record contains an empty or duplicate field name".to_owned());
                }
                validate_type(&field.ty, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Tuple(elements) => {
            validate_types(elements, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Function {
            input,
            output,
            effects,
        } => {
            validate_types(input, scope, allow_unbound_vars)?;
            validate_type(output, scope, allow_unbound_vars)?;
            if let Some(row) = effects {
                validate_effect_row(row, scope)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Handler {
            handled,
            produced,
            result,
        } => {
            validate_effect_row(handled, scope)?;
            if let Some(produced) = produced {
                validate_effect_row(produced, scope)?;
            }
            if let Some(result) = result {
                validate_type(result, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Trust { wrapper, inner } => {
            validate_name(wrapper, "trust wrapper")?;
            validate_type(inner, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Prompt | ProjectExternalTypeInput::PromptPart => Ok(()),
        ProjectExternalTypeInput::ResourceHandle { name, args } => {
            validate_name(name, "resource handle name")?;
            validate_types(args, scope, allow_unbound_vars)
        }
    }
}

fn validate_types(
    types: &[ProjectExternalTypeInput],
    scope: &GenericScope,
    allow_unbound_vars: bool,
) -> Result<(), String> {
    for ty in types {
        validate_type(ty, scope, allow_unbound_vars)?;
    }
    Ok(())
}

fn validate_effect_row(
    row: &ProjectExternalEffectRowInput,
    scope: &GenericScope,
) -> Result<(), String> {
    if let Some(tail) = &row.tail
        && (tail.is_empty() || !scope.effects.contains(tail))
    {
        return Err(format!(
            "effect-row tail `{tail}` does not name a declared effect parameter"
        ));
    }
    for effect in &row.effects {
        validate_effect_ref(effect, scope)?;
    }
    Ok(())
}

fn validate_effect_ref(
    effect: &ProjectExternalEffectRefInput,
    scope: &GenericScope,
) -> Result<(), String> {
    validate_path(&effect.path, "effect path")?;
    for arg in &effect.args {
        validate_effect_arg(arg, scope)?;
    }
    Ok(())
}

fn validate_effect_arg(
    arg: &ProjectExternalEffectArgInput,
    scope: &GenericScope,
) -> Result<(), String> {
    match arg {
        ProjectExternalEffectArgInput::Type(ty) => validate_type(ty, scope, false),
        ProjectExternalEffectArgInput::Path(path) => validate_path(path, "effect selector path"),
        ProjectExternalEffectArgInput::String(_) | ProjectExternalEffectArgInput::Wildcard => {
            Ok(())
        }
        ProjectExternalEffectArgInput::Int(value) => {
            validate_name(value, "integer effect selector")
        }
    }
}

fn validate_path(path: &[String], kind: &str) -> Result<(), String> {
    if path.is_empty() || path.iter().any(String::is_empty) {
        Err(format!("{kind} must contain only non-empty segments"))
    } else {
        Ok(())
    }
}

fn validate_name(name: &str, kind: &str) -> Result<(), String> {
    if name.is_empty() {
        Err(format!("{kind} must not be empty"))
    } else {
        Ok(())
    }
}
