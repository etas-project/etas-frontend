use std::collections::HashSet;

use etas_core::{Diagnostic, TypeDiagnosticCode};

use crate::{
    ExternalCallableGenericParamInput, ExternalCallableGenericParamKindInput,
    ExternalEffectArgInput, ExternalEffectRowInput, ExternalPackageKey,
    ExternalPublicMetadataInput, ExternalSignatureInput, ExternalTypeInput,
    pipeline::context::TypePipelineContext,
};

pub(super) struct ValidatedExternalMetadata {
    packages: HashSet<ExternalPackageKey>,
}

impl ValidatedExternalMetadata {
    pub(super) fn validate(
        ctx: &mut TypePipelineContext<'_>,
        input: &ExternalSignatureInput,
    ) -> Self {
        let mut packages = HashSet::new();
        for metadata in &input.metadata {
            let Some(span) = input
                .symbol_bindings
                .iter()
                .find(|binding| binding.package == metadata.package)
                .map(|binding| binding.span)
                .or_else(|| {
                    input
                        .action_bindings
                        .iter()
                        .find(|binding| binding.package == metadata.package)
                        .map(|binding| binding.span)
                })
            else {
                continue;
            };
            match validate_package(metadata) {
                Ok(()) => {
                    packages.insert(metadata.package);
                }
                Err(reason) => ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    span,
                    format!("invalid external package metadata: {reason}"),
                )),
            }
        }
        Self { packages }
    }

    pub(super) fn contains(&self, package: ExternalPackageKey) -> bool {
        self.packages.contains(&package)
    }
}

fn validate_package(metadata: &ExternalPublicMetadataInput) -> Result<(), String> {
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
            validate_type(ty, &HashSet::new(), &HashSet::new())?;
        }
    }
    for flow in &metadata.flows {
        validate_callable(
            &flow.path,
            &flow.generic_params,
            &flow.params,
            &flow.output,
            flow.effects.as_ref(),
        )?;
    }
    for agent in &metadata.agents {
        validate_callable(
            &agent.path,
            &agent.generic_params,
            &agent.input,
            &agent.output,
            agent.effects.as_ref(),
        )?;
    }
    for tool in &metadata.tools {
        validate_callable(
            &tool.path,
            &tool.generic_params,
            &tool.input,
            &tool.output,
            tool.effects.as_ref(),
        )?;
    }
    for action in &metadata.actions {
        validate_callable(
            &action.path,
            &action.generic_params,
            &action.params,
            &action.output,
            None,
        )?;
        let (type_params, effect_params) = generic_scopes(&action.path, &action.generic_params)?;
        for default in action.selector_defaults.iter().flatten() {
            validate_effect_arg(default, &type_params, &effect_params)?;
        }
    }
    for signature in &metadata.spec_signatures {
        validate_path(&signature.path, "spec path")?;
        for bound in &signature.super_specs {
            validate_path(&bound.spec, "super spec path")?;
            for arg in &bound.args {
                validate_type(arg, &HashSet::new(), &HashSet::new())?;
            }
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
        validate_type(&implementation.self_type, &HashSet::new(), &HashSet::new())?;
        for arg in &implementation.args {
            validate_type(arg, &HashSet::new(), &HashSet::new())?;
        }
    }
    for satisfaction in &metadata.type_spec_satisfactions {
        validate_path(&satisfaction.spec, "type satisfaction spec path")?;
        validate_type(&satisfaction.self_type, &HashSet::new(), &HashSet::new())?;
        for arg in &satisfaction.args {
            validate_type(arg, &HashSet::new(), &HashSet::new())?;
        }
    }
    for satisfaction in &metadata.callable_spec_satisfactions {
        validate_path(&satisfaction.item, "callable satisfaction item path")?;
        validate_path(&satisfaction.spec, "callable satisfaction spec path")?;
        for arg in &satisfaction.args {
            validate_type(arg, &HashSet::new(), &HashSet::new())?;
        }
    }
    for conformance in &metadata.trace_spec_conformances {
        validate_path(&conformance.item, "trace conformance item path")?;
        if let crate::ExternalTraceSpecConformanceTargetInput::Named { spec, args } =
            &conformance.target
        {
            validate_path(spec, "trace conformance spec path")?;
            for arg in args {
                validate_type(arg, &HashSet::new(), &HashSet::new())?;
            }
        }
    }
    Ok(())
}

fn validate_callable(
    path: &[String],
    params: &[ExternalCallableGenericParamInput],
    inputs: &[ExternalTypeInput],
    output: &ExternalTypeInput,
    effects: Option<&ExternalEffectRowInput>,
) -> Result<(), String> {
    validate_path(path, "callable path")?;
    let (type_params, effect_params) = generic_scopes(path, params)?;
    for param in params {
        for bound in &param.bounds {
            validate_path(&bound.spec, "generic bound spec path")?;
            for arg in &bound.args {
                validate_type(arg, &type_params, &effect_params)?;
            }
        }
    }
    for input in inputs {
        validate_type(input, &type_params, &effect_params)?;
    }
    validate_type(output, &type_params, &effect_params)?;
    if let Some(row) = effects {
        validate_effect_row(row, &type_params, &effect_params)?;
    }
    Ok(())
}

fn generic_scopes(
    path: &[String],
    params: &[ExternalCallableGenericParamInput],
) -> Result<(HashSet<String>, HashSet<String>), String> {
    let mut names = HashSet::new();
    let mut type_params = HashSet::new();
    let mut effect_params = HashSet::new();
    for param in params {
        if param.name.is_empty() || !names.insert(param.name.clone()) {
            return Err(format!(
                "callable `{}` has an empty or duplicate generic parameter",
                path.join(".")
            ));
        }
        match param.kind {
            ExternalCallableGenericParamKindInput::Type => {
                type_params.insert(param.name.clone());
            }
            ExternalCallableGenericParamKindInput::Effect => {
                effect_params.insert(param.name.clone());
            }
        }
    }
    Ok((type_params, effect_params))
}

fn validate_type(
    ty: &ExternalTypeInput,
    type_params: &HashSet<String>,
    effect_params: &HashSet<String>,
) -> Result<(), String> {
    match ty {
        ExternalTypeInput::Primitive(name) => validate_name(name, "primitive name"),
        ExternalTypeInput::Var(name) => {
            validate_name(name, "type variable")?;
            if !type_params.is_empty() && !type_params.contains(name) {
                return Err(format!(
                    "type variable `{name}` is not declared by the callable"
                ));
            }
            Ok(())
        }
        ExternalTypeInput::Named(path) => validate_path(path, "named type path"),
        ExternalTypeInput::Applied { path, args } => {
            validate_path(path, "applied type path")?;
            validate_types(args, type_params, effect_params)
        }
        ExternalTypeInput::Alias { path, target } => {
            validate_path(path, "alias path")?;
            validate_type(target, type_params, effect_params)
        }
        ExternalTypeInput::Nominal {
            path,
            representation,
        } => {
            validate_path(path, "nominal type path")?;
            if let Some(representation) = representation {
                validate_type(representation, type_params, effect_params)?;
            }
            Ok(())
        }
        ExternalTypeInput::Array(inner)
        | ExternalTypeInput::List(inner)
        | ExternalTypeInput::Set(inner)
        | ExternalTypeInput::Range(inner)
        | ExternalTypeInput::Slice(inner)
        | ExternalTypeInput::Option(inner)
        | ExternalTypeInput::Message(inner)
        | ExternalTypeInput::MemorySelection(inner)
        | ExternalTypeInput::MemoryRegion(inner) => {
            validate_type(inner, type_params, effect_params)
        }
        ExternalTypeInput::Map { key, value }
        | ExternalTypeInput::Store { key, value }
        | ExternalTypeInput::Result {
            ok: key,
            err: value,
        } => {
            validate_type(key, type_params, effect_params)?;
            validate_type(value, type_params, effect_params)
        }
        ExternalTypeInput::Record { fields } => {
            let mut names = HashSet::new();
            for field in fields {
                if field.name.is_empty() || !names.insert(field.name.as_str()) {
                    return Err("record contains an empty or duplicate field name".to_owned());
                }
                validate_type(&field.ty, type_params, effect_params)?;
            }
            Ok(())
        }
        ExternalTypeInput::Tuple(elements) => validate_types(elements, type_params, effect_params),
        ExternalTypeInput::Function {
            input,
            output,
            effects,
        } => {
            validate_types(input, type_params, effect_params)?;
            validate_type(output, type_params, effect_params)?;
            if let Some(row) = effects {
                validate_effect_row(row, type_params, effect_params)?;
            }
            Ok(())
        }
        ExternalTypeInput::Handler {
            handled,
            produced,
            result,
        } => {
            validate_effect_row(handled, type_params, effect_params)?;
            if let Some(produced) = produced {
                validate_effect_row(produced, type_params, effect_params)?;
            }
            if let Some(result) = result {
                validate_type(result, type_params, effect_params)?;
            }
            Ok(())
        }
        ExternalTypeInput::Trust { wrapper, inner } => {
            validate_name(wrapper, "trust wrapper")?;
            validate_type(inner, type_params, effect_params)
        }
        ExternalTypeInput::Prompt | ExternalTypeInput::PromptPart => Ok(()),
        ExternalTypeInput::ResourceHandle { name, args } => {
            validate_name(name, "resource handle name")?;
            validate_types(args, type_params, effect_params)
        }
    }
}

fn validate_types(
    types: &[ExternalTypeInput],
    type_params: &HashSet<String>,
    effect_params: &HashSet<String>,
) -> Result<(), String> {
    for ty in types {
        validate_type(ty, type_params, effect_params)?;
    }
    Ok(())
}

fn validate_effect_row(
    row: &ExternalEffectRowInput,
    type_params: &HashSet<String>,
    effect_params: &HashSet<String>,
) -> Result<(), String> {
    if let Some(tail) = &row.tail
        && (tail.is_empty() || !effect_params.contains(tail))
    {
        return Err(format!(
            "effect-row tail `{tail}` does not name a declared effect parameter"
        ));
    }
    for effect in &row.effects {
        validate_path(&effect.path, "effect path")?;
        for arg in &effect.args {
            validate_effect_arg(arg, type_params, effect_params)?;
        }
    }
    Ok(())
}

fn validate_effect_arg(
    arg: &ExternalEffectArgInput,
    type_params: &HashSet<String>,
    effect_params: &HashSet<String>,
) -> Result<(), String> {
    match arg {
        ExternalEffectArgInput::Type(ty) => validate_type(ty, type_params, effect_params),
        ExternalEffectArgInput::Path(path) => validate_path(path, "effect selector path"),
        ExternalEffectArgInput::String(_) | ExternalEffectArgInput::Wildcard => Ok(()),
        ExternalEffectArgInput::Int(value) => validate_name(value, "integer effect selector"),
    }
}

fn validate_path(path: &[String], kind: &str) -> Result<(), String> {
    if path.is_empty() || path.iter().any(String::is_empty) {
        return Err(format!("{kind} must contain only non-empty segments"));
    }
    Ok(())
}

fn validate_name(name: &str, kind: &str) -> Result<(), String> {
    if name.is_empty() {
        Err(format!("{kind} must not be empty"))
    } else {
        Ok(())
    }
}
