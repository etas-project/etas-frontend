use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ExternalPackageId, ImportTarget, ProjectContext, ProjectEnvironmentInput,
    ProjectExternalActionTraceInput, ProjectExternalCallableGenericParamInput,
    ProjectExternalCallableGenericParamKindInput, ProjectExternalEffectArgInput,
    ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
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
        let environment = context.input.environment.clone();
        if let Err(error) = validate_external_environment(&environment) {
            let Some(package) = error.package else {
                return PassResult::failed(format!(
                    "invalid external environment has no package identity: {}",
                    error.reason
                ));
            };
            let Some(span) = external_package_import_span(context, package) else {
                return PassResult::failed(format!(
                    "invalid external package metadata for package {} has no resolved import anchor: {}",
                    package.0, error.reason
                ));
            };
            context.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                format!("invalid external package metadata: {}", error.reason),
            ));
            context.validated_external_environment = None;
            return PassResult::stop();
        }
        context.validated_external_environment =
            Some(ValidatedExternalEnvironment::new(environment));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(VALIDATED_EXTERNAL_ENVIRONMENT),
        )
    }
}

#[derive(Clone, Debug)]
struct ExternalEnvironmentValidationError {
    package: Option<ExternalPackageId>,
    reason: String,
}

impl ExternalEnvironmentValidationError {
    fn package(package: ExternalPackageId, reason: impl Into<String>) -> Self {
        Self {
            package: Some(package),
            reason: reason.into(),
        }
    }

    fn global(reason: impl Into<String>) -> Self {
        Self {
            package: None,
            reason: reason.into(),
        }
    }
}

fn validate_external_environment(
    environment: &ProjectEnvironmentInput,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut packages = BTreeMap::<ExternalPackageId, &crate::ProjectExternalPackageInput>::new();
    let mut import_roots = BTreeMap::<String, ExternalPackageId>::new();
    for package in &environment.external_packages {
        if package.name.is_empty()
            || package.version.is_empty()
            || package.edition.is_empty()
            || package.import_root.is_empty()
        {
            return Err(ExternalEnvironmentValidationError::package(
                package.id,
                "external package identity fields must not be empty",
            ));
        }
        if packages.insert(package.id, package).is_some() {
            return Err(ExternalEnvironmentValidationError::package(
                package.id,
                format!("external package id {} is duplicated", package.id.0),
            ));
        }
        if let Some(existing) = import_roots.insert(package.import_root.clone(), package.id) {
            return Err(ExternalEnvironmentValidationError::package(
                package.id,
                format!(
                    "external import root `{}` is shared by packages {} and {}",
                    package.import_root, existing.0, package.id.0
                ),
            ));
        }
    }

    let mut module_ids = BTreeSet::new();
    let mut module_paths = BTreeMap::<Vec<String>, ExternalPackageId>::new();
    let mut exported_paths = BTreeMap::<ExternalPackageId, BTreeSet<Vec<String>>>::new();
    for module in &environment.external_modules {
        let Some(package) = module.package else {
            return Err(ExternalEnvironmentValidationError::global(format!(
                "external module `{}` has no package owner",
                module.path.segments.join(".")
            )));
        };
        let Some(package_record) = packages.get(&package) else {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                format!(
                    "external module `{}` references unknown package {}",
                    module.path.segments.join("."),
                    package.0
                ),
            ));
        };
        validate_path(&module.path.segments, "external module path")
            .map_err(|reason| ExternalEnvironmentValidationError::package(package, reason))?;
        let import_root = package_record
            .import_root
            .split('.')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !module.path.segments.starts_with(&import_root) {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                format!(
                    "external module `{}` is outside import root `{}`",
                    module.path.segments.join("."),
                    package_record.import_root
                ),
            ));
        }
        if !module_ids.insert((package, module.id)) {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                format!("external module id {} is duplicated", module.id.0),
            ));
        }
        if let Some(existing) = module_paths.insert(module.path.segments.clone(), package) {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                format!(
                    "external module path `{}` is shared by packages {} and {}",
                    module.path.segments.join("."),
                    existing.0,
                    package.0
                ),
            ));
        }
        let mut export_names = BTreeSet::new();
        let mut export_symbols = BTreeSet::new();
        for export in &module.exports {
            if export.name.is_empty() {
                return Err(ExternalEnvironmentValidationError::package(
                    package,
                    format!(
                        "external module `{}` contains an empty export name",
                        module.path.segments.join(".")
                    ),
                ));
            }
            if !export_names.insert(export.name.clone()) || !export_symbols.insert(export.symbol) {
                return Err(ExternalEnvironmentValidationError::package(
                    package,
                    format!(
                        "external module `{}` contains duplicate export identity for `{}`",
                        module.path.segments.join("."),
                        export.name
                    ),
                ));
            }
            let mut path = module.path.segments.clone();
            path.push(export.name.clone());
            exported_paths.entry(package).or_default().insert(path);
        }
    }

    let mut metadata_packages = BTreeSet::new();
    let mut declared_paths = BTreeMap::<ExternalPackageId, BTreeSet<Vec<String>>>::new();
    let mut effect_paths = BTreeSet::<Vec<String>>::new();
    let mut action_paths = BTreeSet::<Vec<String>>::new();
    let mut tool_paths = BTreeSet::<Vec<String>>::new();
    for metadata in &environment.external_public_metadata {
        if !packages.contains_key(&metadata.package) {
            return Err(ExternalEnvironmentValidationError::package(
                metadata.package,
                format!(
                    "public metadata references unknown package {}",
                    metadata.package.0
                ),
            ));
        }
        if !metadata_packages.insert(metadata.package) {
            return Err(ExternalEnvironmentValidationError::package(
                metadata.package,
                "external package has duplicate public metadata records",
            ));
        }
        validate_external_metadata(metadata).map_err(|reason| {
            ExternalEnvironmentValidationError::package(metadata.package, reason)
        })?;
        let declarations = declared_export_paths(metadata);
        declared_paths
            .entry(metadata.package)
            .or_default()
            .extend(declarations);
        effect_paths.extend(metadata.effects.iter().map(|item| item.path.clone()));
        action_paths.extend(metadata.actions.iter().map(|item| item.path.clone()));
        tool_paths.extend(metadata.tools.iter().map(|item| item.path.clone()));
        for action in &metadata.actions {
            let owner = &action.path[..action.path.len().saturating_sub(1)];
            if owner.is_empty() || !effect_paths.contains(owner) {
                return Err(ExternalEnvironmentValidationError::package(
                    metadata.package,
                    format!(
                        "external action `{}` references an undeclared effect owner",
                        action.path.join(".")
                    ),
                ));
            }
        }
    }

    for (package, exports) in &exported_paths {
        let declarations = declared_paths.get(package).cloned().unwrap_or_default();
        let re_exports = environment
            .external_public_metadata
            .iter()
            .filter(|metadata| metadata.package == *package)
            .flat_map(|metadata| metadata.re_exports.iter().map(|item| item.exported.clone()))
            .collect::<BTreeSet<_>>();
        for export in exports {
            if !declarations.contains(export) && !re_exports.contains(export) {
                return Err(ExternalEnvironmentValidationError::package(
                    *package,
                    format!(
                        "external export `{}` has no matching public metadata identity",
                        export.join(".")
                    ),
                ));
            }
        }
    }
    for (package, declarations) in &declared_paths {
        let exports = exported_paths.get(package).cloned().unwrap_or_default();
        let re_export_sources = environment
            .external_public_metadata
            .iter()
            .filter(|metadata| metadata.package == *package)
            .flat_map(|metadata| metadata.re_exports.iter().map(|item| item.from.clone()))
            .collect::<BTreeSet<_>>();
        for declaration in declarations {
            if !exports.contains(declaration) && !re_export_sources.contains(declaration) {
                return Err(ExternalEnvironmentValidationError::package(
                    *package,
                    format!(
                        "external declaration `{}` has no matching module export identity",
                        declaration.join(".")
                    ),
                ));
            }
        }
    }

    validate_dependency_effect_metadata(environment, &packages, &effect_paths, &action_paths)?;
    validate_tool_bindings(environment, &packages, &tool_paths)?;
    Ok(())
}

fn declared_export_paths(metadata: &ProjectExternalPublicMetadataInput) -> BTreeSet<Vec<String>> {
    metadata
        .types
        .iter()
        .map(|item| item.path.clone())
        .chain(metadata.values.iter().map(|item| item.path.clone()))
        .chain(metadata.enums.iter().map(|item| item.path.clone()))
        .chain(metadata.flows.iter().map(|item| item.path.clone()))
        .chain(metadata.agents.iter().map(|item| item.path.clone()))
        .chain(metadata.tools.iter().map(|item| item.path.clone()))
        .chain(metadata.effects.iter().map(|item| item.path.clone()))
        .chain(metadata.trace_specs.iter().map(|item| item.path.clone()))
        .chain(
            metadata
                .spec_signatures
                .iter()
                .map(|item| item.path.clone()),
        )
        .collect()
}

fn validate_dependency_effect_metadata(
    environment: &ProjectEnvironmentInput,
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
    public_effect_paths: &BTreeSet<Vec<String>>,
    public_action_paths: &BTreeSet<Vec<String>>,
) -> Result<(), ExternalEnvironmentValidationError> {
    let metadata = &environment.external_effect_metadata;
    let mut tags = BTreeMap::<Vec<String>, Option<&etas_effects::RuntimeRequirementReason>>::new();
    for tag in &metadata.tags {
        validate_path(&tag.path, "dependency effect tag path")
            .map_err(|reason| external_path_error(packages, &tag.path, reason))?;
        if let Some(existing) = tags.insert(tag.path.clone(), tag.runtime_requirement.as_ref())
            && existing != tag.runtime_requirement.as_ref()
        {
            return Err(external_path_error(
                packages,
                &tag.path,
                format!(
                    "dependency effect tag `{}` has conflicting declarations",
                    tag.path.join(".")
                ),
            ));
        }
    }
    let mut known_tags = tags.keys().cloned().collect::<BTreeSet<_>>();
    known_tags.extend(public_effect_paths.iter().cloned());

    let mut actions = BTreeSet::new();
    for action in &metadata.actions {
        validate_path(&action.path, "dependency effect action path")
            .map_err(|reason| external_path_error(packages, &action.path, reason))?;
        if !actions.insert(action.path.clone()) || public_action_paths.contains(&action.path) {
            return Err(external_path_error(
                packages,
                &action.path,
                format!(
                    "dependency effect action `{}` is duplicated",
                    action.path.join(".")
                ),
            ));
        }
        if action.effect_args.len() != action.selector_param_names.len()
            || action.effect_args.len() != action.selector_defaults.len()
        {
            return Err(external_path_error(
                packages,
                &action.path,
                format!(
                    "dependency action `{}` selector metadata lengths do not match",
                    action.path.join(".")
                ),
            ));
        }
        for (index, (kind, default)) in action
            .effect_args
            .iter()
            .zip(&action.selector_defaults)
            .enumerate()
        {
            if default
                .as_ref()
                .is_some_and(|default| !dependency_selector_default_matches_kind(default, kind))
            {
                return Err(external_path_error(
                    packages,
                    &action.path,
                    format!(
                        "dependency action `{}` selector default at index {index} does not match selector kind",
                        action.path.join(".")
                    ),
                ));
            }
        }
        let owner = &action.path[..action.path.len().saturating_sub(1)];
        if owner.is_empty() || !known_tags.contains(owner) {
            return Err(external_path_error(
                packages,
                &action.path,
                format!(
                    "dependency action `{}` references an unknown effect owner",
                    action.path.join(".")
                ),
            ));
        }
    }
    for extension in &metadata.extensions {
        validate_path(&extension.child, "dependency effect extension child")
            .map_err(|reason| external_path_error(packages, &extension.child, reason))?;
        validate_path(&extension.parent, "dependency effect extension parent")
            .map_err(|reason| external_path_error(packages, &extension.parent, reason))?;
        let extension_package = extension
            .package
            .map(ExternalPackageId)
            .or_else(|| package_for_external_path(packages, &extension.child));
        if !known_tags.contains(&extension.child) {
            return Err(ExternalEnvironmentValidationError {
                package: extension_package,
                reason: format!(
                    "dependency effect extension child `{}` is not declared",
                    extension.child.join(".")
                ),
            });
        }
        if let Some(package) = extension.package.map(ExternalPackageId)
            && !packages.contains_key(&package)
        {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                "dependency effect extension references an unknown package",
            ));
        }
    }
    Ok(())
}

fn validate_tool_bindings(
    environment: &ProjectEnvironmentInput,
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
    public_tool_paths: &BTreeSet<Vec<String>>,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut seen = BTreeSet::new();
    for binding in &environment.tool_bindings {
        let path = binding
            .tool
            .split('.')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        validate_path(&path, "tool binding path")
            .map_err(|reason| external_path_error(packages, &path, reason))?;
        if binding.provider.is_empty()
            || binding.effect_row.iter().any(|item| item.trim().is_empty())
            || binding.action_row.iter().any(|item| item.trim().is_empty())
        {
            return Err(external_path_error(
                packages,
                &path,
                format!(
                    "tool binding `{}` contains an empty provider or effect/action entry",
                    binding.tool
                ),
            ));
        }
        if !seen.insert(path.clone()) {
            return Err(external_path_error(
                packages,
                &path,
                format!("tool binding `{}` is duplicated", binding.tool),
            ));
        }
        if !public_tool_paths.contains(&path) {
            if let Some(package) = package_for_external_path(packages, &path) {
                return Err(ExternalEnvironmentValidationError::package(
                    package,
                    format!(
                        "tool binding `{}` does not name a declared external tool",
                        binding.tool
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn external_path_error(
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
    path: &[String],
    reason: impl Into<String>,
) -> ExternalEnvironmentValidationError {
    ExternalEnvironmentValidationError {
        package: package_for_external_path(packages, path),
        reason: reason.into(),
    }
}

fn package_for_external_path(
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
    path: &[String],
) -> Option<ExternalPackageId> {
    packages
        .values()
        .filter_map(|package| {
            let root = package.import_root.split('.').collect::<Vec<_>>();
            path.iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .starts_with(&root)
                .then_some((root.len(), package.id))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, package)| package)
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
    let mut declaration_paths = HashSet::new();
    for item in metadata
        .types
        .iter()
        .chain(&metadata.values)
        .chain(&metadata.enums)
        .chain(&metadata.effects)
        .chain(&metadata.trace_specs)
    {
        validate_path(&item.path, "export path")?;
        validate_visibility(&item.visibility)?;
        register_declaration(&mut declaration_paths, &item.path)?;
        if let Some(ty) = &item.ty {
            validate_type(ty, &GenericScope::default(), true)?;
        }
    }
    for flow in &metadata.flows {
        validate_visibility(&flow.visibility)?;
        register_declaration(&mut declaration_paths, &flow.path)?;
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
        validate_visibility(&agent.visibility)?;
        register_declaration(&mut declaration_paths, &agent.path)?;
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
        validate_visibility(&tool.visibility)?;
        register_declaration(&mut declaration_paths, &tool.path)?;
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
        if !metadata.tools.iter().any(|tool| tool.path == schema.path) {
            return Err(format!(
                "tool schema `{}` has no matching tool declaration",
                schema.path.join(".")
            ));
        }
        serde_json::from_str::<serde_json::Value>(&schema.schema_json)
            .map_err(|error| format!("tool schema is not valid JSON: {error}"))?;
    }
    for action in &metadata.actions {
        validate_visibility(&action.visibility)?;
        register_declaration(&mut declaration_paths, &action.path)?;
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
        for (index, (kind, default)) in action
            .effect_args
            .iter()
            .zip(&action.selector_defaults)
            .enumerate()
        {
            if default
                .as_ref()
                .is_some_and(|default| !external_selector_default_matches_kind(default, kind))
            {
                return Err(format!(
                    "action `{}` selector default at index {index} does not match selector kind",
                    action.path.join(".")
                ));
            }
        }
        for (kind, name) in action.effect_args.iter().zip(&action.selector_param_names) {
            if name.is_empty()
                || matches!(kind, crate::ProjectExternalActionArgKindInput::Type)
                    && !scope.types.contains(name)
            {
                return Err(format!(
                    "action `{}` selector `{name}` does not name a compatible generic parameter",
                    action.path.join(".")
                ));
            }
        }
    }
    for value in &metadata.values {
        callable_scopes.entry(value.path.clone()).or_default();
    }
    validate_spec_facts(metadata)?;
    validate_effect_summaries(metadata, &callable_scopes)?;
    for summary in &metadata.action_summaries {
        validate_path(&summary.action, "action summary path")?;
        if !metadata
            .actions
            .iter()
            .any(|action| action.path == summary.action)
        {
            return Err(format!(
                "action summary `{}` has no matching action declaration",
                summary.action.join(".")
            ));
        }
        if summary.args.iter().any(String::is_empty) {
            return Err(format!(
                "action summary `{}` contains an empty selector",
                summary.action.join(".")
            ));
        }
    }
    for summary in &metadata.trace_spec_summaries {
        validate_path(&summary.trace_spec, "trace spec summary path")?;
        if !metadata
            .trace_specs
            .iter()
            .any(|spec| spec.path == summary.trace_spec)
            && !metadata
                .spec_signatures
                .iter()
                .any(|spec| spec.path == summary.trace_spec)
        {
            return Err(format!(
                "trace spec summary `{}` has no matching trace spec declaration",
                summary.trace_spec.join(".")
            ));
        }
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
    let mut paths = HashSet::new();
    for signature in &metadata.spec_signatures {
        validate_path(&signature.path, "spec path")?;
        validate_visibility(&signature.visibility)?;
        if !paths.insert(signature.path.clone()) {
            return Err(format!(
                "external spec `{}` is declared more than once",
                signature.path.join(".")
            ));
        }
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

fn register_declaration(paths: &mut HashSet<Vec<String>>, path: &[String]) -> Result<(), String> {
    if paths.insert(path.to_vec()) {
        Ok(())
    } else {
        Err(format!(
            "external declaration `{}` is defined more than once",
            path.join(".")
        ))
    }
}

fn validate_visibility(visibility: &str) -> Result<(), String> {
    match visibility {
        "public" | "private" => Ok(()),
        other => Err(format!("external visibility `{other}` is not supported")),
    }
}

fn external_selector_default_matches_kind(
    default: &ProjectExternalEffectArgInput,
    kind: &crate::ProjectExternalActionArgKindInput,
) -> bool {
    if matches!(default, ProjectExternalEffectArgInput::Wildcard) {
        return true;
    }
    match kind {
        crate::ProjectExternalActionArgKindInput::Type => {
            matches!(default, ProjectExternalEffectArgInput::Type(_))
        }
        crate::ProjectExternalActionArgKindInput::MemoryPlace
        | crate::ProjectExternalActionArgKindInput::StaticResourcePath { .. } => {
            matches!(default, ProjectExternalEffectArgInput::Path(_))
        }
        crate::ProjectExternalActionArgKindInput::StringPattern => matches!(
            default,
            ProjectExternalEffectArgInput::String(_)
                | ProjectExternalEffectArgInput::Int(_)
                | ProjectExternalEffectArgInput::Path(_)
        ),
    }
}

fn dependency_selector_default_matches_kind(
    default: &etas_types::EffectArgRef,
    kind: &etas_effects::EffectActionArgKind,
) -> bool {
    if matches!(default, etas_types::EffectArgRef::Wildcard) {
        return true;
    }
    match kind {
        etas_effects::EffectActionArgKind::Type => {
            matches!(default, etas_types::EffectArgRef::Type(_))
        }
        etas_effects::EffectActionArgKind::MemoryPlace
        | etas_effects::EffectActionArgKind::StaticResourcePath { .. } => {
            matches!(default, etas_types::EffectArgRef::Path(_))
        }
        etas_effects::EffectActionArgKind::StringPattern => matches!(
            default,
            etas_types::EffectArgRef::String(_)
                | etas_types::EffectArgRef::Int(_)
                | etas_types::EffectArgRef::Path(_)
        ),
    }
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
