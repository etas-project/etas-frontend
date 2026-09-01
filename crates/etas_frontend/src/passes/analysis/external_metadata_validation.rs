use std::collections::{HashMap, HashSet};

use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ExternalPackageId, ImportTarget, ProjectContext, ProjectExternalActionTraceInput,
    ProjectExternalCallableGenericParamInput, ProjectExternalCallableGenericParamKindInput,
    ProjectExternalEffectArgInput, ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalPublicMetadataInput, ProjectExternalTypeInput, ResolvedModuleTarget,
    ValidatedExternalEnvironment,
};

use crate::passes::artifacts::{
    RESOLVED_IMPORTS, VALIDATED_EXTERNAL_ENVIRONMENT, global_with_diagnostics,
};

pub struct ValidateExternalEnvironmentPass;

impl Pass<ProjectContext> for ValidateExternalEnvironmentPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateExternalEnvironmentPass", PassKind::Analysis)
            .requires(ArtifactSet::one(RESOLVED_IMPORTS))
            .produces(global_with_diagnostics([VALIDATED_EXTERNAL_ENVIRONMENT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let mut environment = context.input.environment.clone();
        let mut validated = Vec::with_capacity(environment.external_public_metadata.len());
        for metadata in std::mem::take(&mut environment.external_public_metadata) {
            match validate_external_metadata(&metadata) {
                Ok(()) => validated.push(metadata),
                Err(reason) => {
                    let Some(span) = external_package_import_span(context, metadata.package) else {
                        return PassResult::failed(format!(
                            "invalid external package metadata for package {} has no resolved import anchor: {reason}",
                            metadata.package.0
                        ));
                    };
                    context.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::IncompleteTypeFacts,
                        span,
                        format!("invalid external package metadata: {reason}"),
                    ));
                }
            }
        }
        environment.external_public_metadata = validated;
        context.validated_external_environment =
            Some(ValidatedExternalEnvironment::new(environment));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(VALIDATED_EXTERNAL_ENVIRONMENT),
        )
    }
}

fn external_package_import_span(
    context: &ProjectContext,
    package: ExternalPackageId,
) -> Option<etas_core::Span> {
    let resolved = context.resolved_imports.as_ref()?;
    resolved
        .imports
        .iter()
        .find_map(|import| match &import.target {
            ImportTarget::ExternalItem {
                package: Some(target),
                ..
            } if *target == package => Some(import.span),
            ImportTarget::Module(ResolvedModuleTarget::External {
                package: Some(target),
                ..
            }) if *target == package => Some(import.span),
            _ => None,
        })
        .or_else(|| {
            resolved
                .wildcard_imports
                .iter()
                .find_map(|import| match &import.target_module {
                    ResolvedModuleTarget::External {
                        package: Some(target),
                        ..
                    } if *target == package => Some(import.span),
                    _ => None,
                })
        })
}

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
    let mut seen = HashSet::new();
    for summary in &metadata.effect_summaries {
        validate_path(&summary.item, "effect summary item path")?;
        if !seen.insert(summary.item.clone()) {
            return Err(format!(
                "external callable `{}` has duplicate solved effect summaries",
                summary.item.join(".")
            ));
        }
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
    let required = metadata
        .flows
        .iter()
        .map(|callable| &callable.path)
        .chain(metadata.agents.iter().map(|callable| &callable.path))
        .chain(metadata.tools.iter().map(|callable| &callable.path))
        .chain(metadata.values.iter().filter_map(|value| {
            matches!(
                value.ty.as_ref(),
                Some(ProjectExternalTypeInput::Function { .. })
                    | Some(ProjectExternalTypeInput::Handler { .. })
            )
            .then_some(&value.path)
        }));
    for item in required {
        if !seen.contains(item) {
            return Err(format!(
                "external callable `{}` does not provide a solved effect summary",
                item.join(".")
            ));
        }
    }
    Ok(())
}

fn validate_action_trace(
    trace: &ProjectExternalActionTraceInput,
    scope: &GenericScope,
    parameter_names: &[String],
) -> Result<(), String> {
    validate_action_trace_resource_bounds(trace)?;
    validate_action_trace_semantics(trace, scope, parameter_names)
}

fn validate_action_trace_semantics(
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
                validate_action_trace_semantics(child, scope, parameter_names)?;
            }
            Ok(())
        }
        ProjectExternalActionTraceInput::Repeat(child) => {
            validate_action_trace_semantics(child, scope, parameter_names)
        }
        ProjectExternalActionTraceInput::UnknownOrder(actions) => {
            for action in actions {
                validate_effect_ref(action, scope)?;
            }
            Ok(())
        }
        ProjectExternalActionTraceInput::Widened {
            actions,
            parameter_calls,
        } => {
            for action in actions {
                validate_effect_ref(action, scope)?;
            }
            for parameter in parameter_calls {
                if !parameter_names.iter().any(|name| name == parameter) {
                    return Err(format!(
                        "action trace parameter `{parameter}` is not declared by the callable"
                    ));
                }
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
    validate_type_resource_bounds(ty)?;
    validate_type_semantics(ty, scope, allow_unbound_vars)
}

fn validate_type_semantics(
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
            validate_types_semantics(args, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Alias { path, target } => {
            validate_path(path, "alias path")?;
            validate_type_semantics(target, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Nominal {
            path,
            representation,
        } => {
            validate_path(path, "nominal type path")?;
            if let Some(representation) = representation {
                validate_type_semantics(representation, scope, allow_unbound_vars)?;
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
            validate_type_semantics(inner, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Map { key, value }
        | ProjectExternalTypeInput::Store { key, value }
        | ProjectExternalTypeInput::Result {
            ok: key,
            err: value,
        } => {
            validate_type_semantics(key, scope, allow_unbound_vars)?;
            validate_type_semantics(value, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Record { fields } => {
            let mut names = HashSet::new();
            for field in fields {
                if field.name.is_empty() || !names.insert(field.name.as_str()) {
                    return Err("record contains an empty or duplicate field name".to_owned());
                }
                validate_type_semantics(&field.ty, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Tuple(elements) => {
            validate_types_semantics(elements, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Function {
            input,
            output,
            effects,
        } => {
            validate_types_semantics(input, scope, allow_unbound_vars)?;
            validate_type_semantics(output, scope, allow_unbound_vars)?;
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
                validate_type_semantics(result, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Trust { wrapper, inner } => {
            validate_name(wrapper, "trust wrapper")?;
            validate_type_semantics(inner, scope, allow_unbound_vars)
        }
        ProjectExternalTypeInput::Prompt | ProjectExternalTypeInput::PromptPart => Ok(()),
        ProjectExternalTypeInput::ResourceHandle { name, args } => {
            validate_name(name, "resource handle name")?;
            validate_types_semantics(args, scope, allow_unbound_vars)
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

fn validate_types_semantics(
    types: &[ProjectExternalTypeInput],
    scope: &GenericScope,
    allow_unbound_vars: bool,
) -> Result<(), String> {
    for ty in types {
        validate_type_semantics(ty, scope, allow_unbound_vars)?;
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

const MAX_EXTERNAL_METADATA_GRAPH_DEPTH: usize = 64;
const MAX_EXTERNAL_METADATA_GRAPH_NODES: usize = 100_000;

enum TypeGraphNode<'a> {
    Type(&'a ProjectExternalTypeInput),
    EffectRow(&'a ProjectExternalEffectRowInput),
    EffectRef(&'a ProjectExternalEffectRefInput),
    EffectArg(&'a ProjectExternalEffectArgInput),
}

fn validate_type_resource_bounds(root: &ProjectExternalTypeInput) -> Result<(), String> {
    let mut stack = vec![(TypeGraphNode::Type(root), 0usize)];
    let mut nodes = 0usize;
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_EXTERNAL_METADATA_GRAPH_DEPTH {
            return Err("external metadata type graph exceeds maximum depth".to_owned());
        }
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| "external metadata type graph node count overflow".to_owned())?;
        if nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err("external metadata type graph exceeds maximum node count".to_owned());
        }
        let next = depth + 1;
        match node {
            TypeGraphNode::Type(ty) => match ty {
                ProjectExternalTypeInput::Applied { args, .. }
                | ProjectExternalTypeInput::Tuple(args)
                | ProjectExternalTypeInput::ResourceHandle { args, .. } => {
                    stack.extend(args.iter().map(|ty| (TypeGraphNode::Type(ty), next)));
                }
                ProjectExternalTypeInput::Alias { target, .. }
                | ProjectExternalTypeInput::Array(target)
                | ProjectExternalTypeInput::List(target)
                | ProjectExternalTypeInput::Set(target)
                | ProjectExternalTypeInput::Range(target)
                | ProjectExternalTypeInput::Slice(target)
                | ProjectExternalTypeInput::Option(target)
                | ProjectExternalTypeInput::Message(target)
                | ProjectExternalTypeInput::MemorySelection(target)
                | ProjectExternalTypeInput::MemoryRegion(target) => {
                    stack.push((TypeGraphNode::Type(target), next));
                }
                ProjectExternalTypeInput::Nominal { representation, .. } => {
                    if let Some(representation) = representation {
                        stack.push((TypeGraphNode::Type(representation), next));
                    }
                }
                ProjectExternalTypeInput::Map { key, value }
                | ProjectExternalTypeInput::Store { key, value }
                | ProjectExternalTypeInput::Result {
                    ok: key,
                    err: value,
                } => {
                    stack.push((TypeGraphNode::Type(key), next));
                    stack.push((TypeGraphNode::Type(value), next));
                }
                ProjectExternalTypeInput::Record { fields } => {
                    stack.extend(
                        fields
                            .iter()
                            .map(|field| (TypeGraphNode::Type(&field.ty), next)),
                    );
                }
                ProjectExternalTypeInput::Function {
                    input,
                    output,
                    effects,
                } => {
                    stack.extend(input.iter().map(|ty| (TypeGraphNode::Type(ty), next)));
                    stack.push((TypeGraphNode::Type(output), next));
                    if let Some(effects) = effects {
                        stack.push((TypeGraphNode::EffectRow(effects), next));
                    }
                }
                ProjectExternalTypeInput::Handler {
                    handled,
                    produced,
                    result,
                } => {
                    stack.push((TypeGraphNode::EffectRow(handled), next));
                    if let Some(produced) = produced {
                        stack.push((TypeGraphNode::EffectRow(produced), next));
                    }
                    if let Some(result) = result {
                        stack.push((TypeGraphNode::Type(result), next));
                    }
                }
                ProjectExternalTypeInput::Trust { inner, .. } => {
                    stack.push((TypeGraphNode::Type(inner), next));
                }
                ProjectExternalTypeInput::Primitive(_)
                | ProjectExternalTypeInput::Var(_)
                | ProjectExternalTypeInput::Named(_)
                | ProjectExternalTypeInput::Prompt
                | ProjectExternalTypeInput::PromptPart => {}
            },
            TypeGraphNode::EffectRow(row) => {
                stack.extend(
                    row.effects
                        .iter()
                        .map(|effect| (TypeGraphNode::EffectRef(effect), next)),
                );
            }
            TypeGraphNode::EffectRef(effect) => {
                stack.extend(
                    effect
                        .args
                        .iter()
                        .map(|arg| (TypeGraphNode::EffectArg(arg), next)),
                );
            }
            TypeGraphNode::EffectArg(ProjectExternalEffectArgInput::Type(ty)) => {
                stack.push((TypeGraphNode::Type(ty), next));
            }
            TypeGraphNode::EffectArg(_) => {}
        }
    }
    Ok(())
}

fn validate_action_trace_resource_bounds(
    root: &ProjectExternalActionTraceInput,
) -> Result<(), String> {
    let mut stack = vec![(root, 0usize)];
    let mut nodes = 0usize;
    while let Some((trace, depth)) = stack.pop() {
        if depth > MAX_EXTERNAL_METADATA_GRAPH_DEPTH {
            return Err("external metadata action trace exceeds maximum depth".to_owned());
        }
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| "external metadata action trace node count overflow".to_owned())?;
        if nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err("external metadata action trace exceeds maximum node count".to_owned());
        }
        let next = depth + 1;
        match trace {
            ProjectExternalActionTraceInput::Seq(children)
            | ProjectExternalActionTraceInput::Choice(children) => {
                stack.extend(children.iter().map(|child| (child, next)));
            }
            ProjectExternalActionTraceInput::Repeat(child) => stack.push((child, next)),
            ProjectExternalActionTraceInput::Empty
            | ProjectExternalActionTraceInput::Event { .. }
            | ProjectExternalActionTraceInput::ParameterCall { .. }
            | ProjectExternalActionTraceInput::UnknownOrder(_)
            | ProjectExternalActionTraceInput::Widened { .. } => {}
        }
    }
    Ok(())
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
