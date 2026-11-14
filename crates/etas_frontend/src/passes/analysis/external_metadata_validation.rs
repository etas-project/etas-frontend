use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ExternalPackageId, ProjectContext, ProjectEnvironmentInput, ProjectExternalActionTraceInput,
    ProjectExternalCallableGenericParamInput, ProjectExternalCallableGenericParamKindInput,
    ProjectExternalEffectArgInput, ProjectExternalEffectRefInput, ProjectExternalEffectRowInput,
    ProjectExternalPublicMetadataInput, ProjectExternalTypeInput, ValidatedExternalEnvironment,
};

use crate::passes::artifacts::{
    MODULE_CATALOG, MODULE_INDEX, RESOLVED_IMPORTS, SOURCE_SET, VALIDATED_EXTERNAL_ENVIRONMENT,
    global_with_diagnostics,
};

#[cfg(test)]
mod scope_tests;
#[cfg(test)]
mod source_origin_tests;

mod dependencies;
mod method_table;
mod origins;
mod requests;
mod specs;
mod trace_aliases;

pub struct ValidateExternalEnvironmentPass;

impl Pass<ProjectContext> for ValidateExternalEnvironmentPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ValidateExternalEnvironmentPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                RESOLVED_IMPORTS,
                SOURCE_SET,
                MODULE_INDEX,
                MODULE_CATALOG,
            ]))
            .produces(global_with_diagnostics([VALIDATED_EXTERNAL_ENVIRONMENT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let environment = context.input.environment.clone();
        let Some(sources) = context.sources.as_ref() else {
            return PassResult::failed("source package validation requires the source set");
        };
        let Some(modules) = context.modules.as_ref() else {
            return PassResult::failed("source package validation requires the module index");
        };
        let validation = validate_source_package_bindings(
            &environment,
            sources.files.iter().map(|source| (source.id, &source.kind)),
        )
        .and_then(|()| validate_module_package_bindings(&environment, sources, modules));
        if let Err(error) = validation {
            let Some(source) = sources
                .files
                .iter()
                .find(|source| source.id == error.source)
            else {
                return PassResult::failed(format!(
                    "source package binding has no source: {}",
                    error.source.0
                ));
            };
            let span = etas_core::Span::new(
                source.id,
                etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(0)),
            );
            context.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                error.reason,
            ));
            context.validated_external_environment = None;
            return PassResult::stop();
        }
        if let Err(error) = validate_external_environment(&environment, &context.std_registry) {
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
        context.validated_external_environment = Some(std::sync::Arc::new(
            ValidatedExternalEnvironment::new(environment),
        ));
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(VALIDATED_EXTERNAL_ENVIRONMENT),
        )
    }
}

fn validate_module_package_bindings(
    environment: &ProjectEnvironmentInput,
    sources: &crate::SourceSet,
    modules: &crate::ModuleIndex,
) -> Result<(), SourcePackageBindingError> {
    let sources_by_id: HashMap<_, _> = sources
        .files
        .iter()
        .map(|source| (source.id, source))
        .collect();
    for (_, module) in modules.modules.iter() {
        let mut owner = None;
        let mut missing = None;
        for part in &module.parts {
            let Some(part) = modules.parts.get(*part) else {
                return Err(SourcePackageBindingError {
                    source: module.span.source,
                    reason: "module has a missing source part",
                });
            };
            let Some(source) = sources_by_id.get(&part.source) else {
                return Err(SourcePackageBindingError {
                    source: part.source,
                    reason: "module part has no source",
                });
            };
            match environment.source_package_for(source.id, &source.kind) {
                Ok(package) if owner.is_some_and(|previous| previous != package) => {
                    return Err(SourcePackageBindingError {
                        source: source.id,
                        reason: "module source parts have conflicting package identities",
                    });
                }
                Ok(package) => owner = Some(package),
                Err(crate::SourcePackageOriginError::Ambiguous) => {
                    return Err(SourcePackageBindingError {
                        source: source.id,
                        reason: "module source part has an ambiguous package identity",
                    });
                }
                Err(crate::SourcePackageOriginError::Conflict) => {
                    return Err(SourcePackageBindingError {
                        source: source.id,
                        reason: "dependency source identity conflicts with its package metadata",
                    });
                }
                Err(
                    crate::SourcePackageOriginError::Missing
                    | crate::SourcePackageOriginError::UnsupportedSource,
                ) => {
                    missing.get_or_insert(source.id);
                }
            }
        }
        if owner.is_some()
            && let Some(source) = missing
        {
            return Err(SourcePackageBindingError {
                source,
                reason: "module source part has no package identity",
            });
        }
    }
    Ok(())
}

struct SourcePackageBindingError {
    source: etas_core::SourceId,
    reason: &'static str,
}

fn validate_source_package_bindings<'a>(
    environment: &ProjectEnvironmentInput,
    source_entries: impl IntoIterator<Item = (etas_core::SourceId, &'a crate::SourceKind)>,
) -> Result<(), SourcePackageBindingError> {
    let mut sources = HashMap::new();
    for (id, kind) in source_entries {
        if sources.insert(id, kind).is_some() {
            return Err(SourcePackageBindingError {
                source: id,
                reason: "source identity is duplicated",
            });
        }
    }
    let mut seen = HashSet::new();
    for binding in &environment.standalone_source_packages {
        let fail = |reason| SourcePackageBindingError {
            source: binding.source,
            reason,
        };
        if !seen.insert(binding.source) {
            return Err(fail("source package identity is duplicated"));
        }
        let source = sources
            .get(&binding.source)
            .ok_or_else(|| fail("source package identity refers to a missing source"))?;
        if environment.local_source_package.is_some() {
            return Err(fail(
                "source package identity conflicts with the local project package",
            ));
        }
        if !matches!(
            source,
            crate::SourceKind::SourceProjectFile | crate::SourceKind::SingleFileInput
        ) {
            return Err(fail(
                "source package identity is attached to a non-local source",
            ));
        }
    }
    Ok(())
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
    std_registry: &etas_std::StdRegistry,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut packages = BTreeMap::<ExternalPackageId, &crate::ProjectExternalPackageInput>::new();
    let mut import_roots = BTreeMap::<String, ExternalPackageId>::new();
    for package in &environment.external_packages {
        if package.name.is_empty()
            || package.version.is_empty()
            || package.edition.is_empty()
            || package.import_root.is_empty()
            || package.std_compatibility.is_empty()
        {
            return Err(ExternalEnvironmentValidationError::package(
                package.id,
                "external package identity fields must not be empty",
            ));
        }
        let expected_std = std_registry.version().compatibility_identity();
        if package.std_compatibility != expected_std {
            return Err(ExternalEnvironmentValidationError::package(
                package.id,
                format!(
                    "external package standard-library compatibility `{}` does not match `{expected_std}`",
                    package.std_compatibility
                ),
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

    let mut source_packages = BTreeSet::new();
    for binding in &environment.dependency_source_packages {
        let package = packages.get(&binding.package).ok_or_else(|| {
            ExternalEnvironmentValidationError::package(
                binding.package,
                "dependency source origin has no package binding",
            )
        })?;
        if !source_packages.insert(binding.package) {
            return Err(ExternalEnvironmentValidationError::package(
                binding.package,
                "dependency source origin is duplicated",
            ));
        }
        if package.import_root != binding.import_root {
            return Err(ExternalEnvironmentValidationError::package(
                binding.package,
                "dependency source origin has a mismatched import root",
            ));
        }
        if environment
            .external_definition_origins
            .iter()
            .any(|origin| {
                origin.package == binding.package
                    && origin.origins.package != binding.definition_package
            })
        {
            return Err(ExternalEnvironmentValidationError::package(
                binding.package,
                "dependency source origin conflicts with external definition metadata",
            ));
        }
    }

    dependencies::validate(environment, &packages)?;
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
        validate_external_metadata(metadata, std_registry).map_err(|reason| {
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

    validate_trace_selector_declarations(environment, std_registry)?;

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

    origins::validate(
        environment,
        &packages,
        &module_paths,
        &exported_paths,
        std_registry,
    )?;
    validate_trace_spec_identities(environment, &packages)?;
    specs::validate(environment)?;
    method_table::validate(environment)?;
    validate_dependency_effect_metadata(environment, &packages, &effect_paths, &action_paths)?;
    validate_tool_bindings(environment, &packages, &tool_paths)?;
    if let Some(metadata) = environment
        .external_public_metadata
        .iter()
        .find(|metadata| {
            metadata.trace_spec_summaries.iter().any(|summary| {
                summary
                    .alternatives
                    .iter()
                    .flatten()
                    .any(|clause| !clause.selector_bounds.is_empty())
            })
        })
    {
        return Err(ExternalEnvironmentValidationError::package(
            metadata.package,
            "trace selector bounds require checked replay support",
        ));
    }
    Ok(())
}

fn validate_trace_spec_identities(
    environment: &ProjectEnvironmentInput,
    packages: &BTreeMap<ExternalPackageId, &crate::ProjectExternalPackageInput>,
) -> Result<(), ExternalEnvironmentValidationError> {
    let mut summaries = BTreeMap::new();
    let mut comparator = trace_aliases::TraceAliasComparator::new(environment);
    let origins = environment
        .external_definition_origins
        .iter()
        .map(|origin| (origin.package, origin))
        .collect::<BTreeMap<_, _>>();
    for metadata in &environment.external_public_metadata {
        if metadata.trace_spec_summaries.is_empty() {
            continue;
        }
        let fail =
            |reason: String| ExternalEnvironmentValidationError::package(metadata.package, reason);
        let package = packages
            .get(&metadata.package)
            .ok_or_else(|| fail("trace spec package is missing".into()))?;
        let key = etas_code::PackageKey::from_name_and_version(&package.name, &package.version)
            .map_err(|error| fail(error.to_string()))?;
        let origin = origins.get(&metadata.package).ok_or_else(|| {
            fail("trace spec summary has no checked package definition origins".into())
        })?;
        if origin.origins.package != key {
            return Err(fail(
                "trace spec package origin does not match its name and version".into(),
            ));
        }
        let modules = origin
            .origins
            .modules
            .iter()
            .map(|module| (module.path.as_slice(), module.canonical_path.as_slice()))
            .collect::<BTreeMap<_, _>>();
        for summary in &metadata.trace_spec_summaries {
            let Some((name, module)) = summary.trace_spec.split_last() else {
                return Err(fail("trace spec summary has an empty path".into()));
            };
            let canonical_module = modules.get(module).ok_or_else(|| {
                fail(format!(
                    "trace spec `{}` has no canonical module origin",
                    summary.trace_spec.join(".")
                ))
            })?;
            if summary.definition.kind() != etas_code::DefinitionKind::Spec
                || summary.definition.package() != key
                || summary.definition.module() != *canonical_module
                || summary.definition.declaration() != std::slice::from_ref(name)
            {
                return Err(fail(format!(
                    "trace spec `{}` has an inconsistent definition identity",
                    summary.trace_spec.join(".")
                )));
            }
            if let Some(previous) = summaries.insert(&summary.definition, &summary.alternatives) {
                if !comparator
                    .alternatives(previous, &summary.alternatives)
                    .map_err(fail)?
                {
                    return Err(fail(
                        "conflicting metadata for trace spec definition identity".into(),
                    ));
                }
            }
        }
    }
    for metadata in &environment.external_public_metadata {
        for conformance in &metadata.trace_spec_conformances {
            if let crate::ProjectExternalTraceSpecConformanceTargetInput::Named {
                definition, ..
            } = &conformance.target
            {
                if definition.kind() != etas_code::DefinitionKind::Spec
                    || !summaries.contains_key(definition)
                {
                    return Err(ExternalEnvironmentValidationError::package(
                        metadata.package,
                        "trace spec conformance has no checked target definition",
                    ));
                }
            }
        }
    }
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
    crate::project::ExternalImportAnchorIndex::build(
        resolved,
        &context.input.environment,
        context.module_catalog.as_ref()?,
    )
    .package_span(package)
}

#[derive(Clone, Debug, Default)]
struct GenericScope {
    parameter_kinds: Vec<etas_package_metadata::GenericParamKind>,
    types: HashSet<String>,
    effects: HashSet<String>,
}

pub(super) fn validate_external_metadata(
    metadata: &ProjectExternalPublicMetadataInput,
    std_registry: &etas_std::StdRegistry,
) -> Result<(), String> {
    let mut callable_scopes = HashMap::<Vec<String>, GenericScope>::new();
    let mut declaration_paths = HashSet::new();
    for declaration in &metadata.enums {
        if !matches!(&declaration.ty, Some(ProjectExternalTypeInput::Enum { path, .. }) if *path == declaration.path)
        {
            return Err(
                "external enum declaration requires its own checked constructor layout".into(),
            );
        }
    }
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
        let scope = generic_scope(&item.path, &item.generic_params)?;
        for param in &item.generic_params {
            for bound in &param.bounds {
                validate_path(&bound.spec, "generic bound spec path")?;
                validate_types(&bound.args, &scope, false)?;
            }
        }
        if let Some(ty) = &item.ty {
            validate_type(ty, &scope, false)?;
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
        validate_path(&agent.path, "agent declaration path")?;
        register_declaration(&mut declaration_paths, &agent.path)?;
        let Some(output) = &agent.output else {
            if !agent.generic_params.is_empty()
                || !agent.param_names.is_empty()
                || !agent.input.is_empty()
                || agent.effects.is_some()
            {
                return Err(
                    "agent without a default entry contains callable signature fields".into(),
                );
            }
            continue;
        };
        let scope = validate_callable(
            &agent.path,
            &agent.generic_params,
            &agent.input,
            output,
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
            if matches!(kind, crate::ProjectExternalActionArgKindInput::Type)
                && (name.is_empty() || !scope.types.contains(name))
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
    validate_effect_summaries(metadata, &callable_scopes, std_registry)?;
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
    let mut trace_selector_nodes = 0usize;
    for summary in &metadata.trace_spec_summaries {
        trace_selector_nodes = trace_selector_nodes
            .checked_add(1)
            .and_then(|nodes| nodes.checked_add(summary.alternatives.len()))
            .ok_or("trace spec graph node count overflow")?;
        for alternative in &summary.alternatives {
            trace_selector_nodes = trace_selector_nodes
                .checked_add(alternative.len())
                .ok_or("trace spec graph node count overflow")?;
        }
        if trace_selector_nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err("trace spec graph exceeds maximum node count".into());
        }
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
                parameter_kinds: vec![
                    etas_package_metadata::GenericParamKind::Type;
                    signature.param_names.len()
                ],
                types: signature.param_names.iter().cloned().collect(),
                effects: HashSet::new(),
            })
            .unwrap_or_default();
        if summary.alternatives.is_empty() || summary.alternatives.iter().any(Vec::is_empty) {
            return Err(format!(
                "trace spec summary `{}` has an empty alternative set or clause group",
                summary.trace_spec.join(".")
            ));
        }
        for clause in summary.alternatives.iter().flatten() {
            validate_trace_selector_bounds(clause, &scope, &mut trace_selector_nodes)?;
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

fn validate_trace_selector_bounds(
    clause: &crate::ProjectExternalTraceSpecClauseInput,
    scope: &GenericScope,
    nodes: &mut usize,
) -> Result<(), String> {
    if clause.selector_bounds.len() > MAX_EXTERNAL_METADATA_GRAPH_NODES {
        return Err("external trace selector bounds exceed maximum node count".into());
    }
    for bound in &clause.selector_bounds {
        *nodes = nodes
            .checked_add(1)
            .ok_or("trace selector graph node count overflow")?;
        if *nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err("trace selector graph exceeds maximum node count".into());
        }
        if bound.spec_args.len() > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err("trace selector spec arguments exceed maximum node count".into());
        }
        validate_name(&bound.binding, "trace selector binding")?;
        if bound.binding == "_" {
            return Err("trace selector binding must not be a wildcard".into());
        }
        validate_path(&bound.effect_path, "trace selector action path")?;
        validate_path(&bound.spec_path, "trace selector spec path")?;
        use crate::{
            ProjectExternalTraceSelectorOperandInput as O,
            ProjectExternalTraceSpecClauseKindInput as K,
        };
        let row = match (clause.kind, bound.operand) {
            (K::Allow | K::Deny, O::Pattern) => &clause.pattern,
            (K::RequireBefore, O::Guard) => &clause.guard,
            (K::RequireBefore | K::RequireAfter, O::Target) => &clause.target,
            (K::RequireAfter, O::Obligation) => &clause.obligation,
            _ => return Err("trace selector bound has an invalid clause operand".into()),
        };
        let mut effects = row
            .as_ref()
            .into_iter()
            .flat_map(|row| &row.effects)
            .filter(|effect| effect.path == bound.effect_path);
        let effect = effects
            .next()
            .ok_or("trace selector bound references a missing action")?;
        if effects.next().is_some() {
            return Err("trace selector bound references an ambiguous action".into());
        }
        let selector = usize::try_from(bound.argument)
            .ok()
            .and_then(|index| effect.args.get(index));
        if !matches!(selector, Some(ProjectExternalEffectArgInput::Wildcard)) {
            return Err("trace selector bound requires a wildcard at its selector position".into());
        }
        let mut bound_scope = scope.clone();
        bound_scope.types.insert(bound.binding.clone());
        for ty in &bound.spec_args {
            validate_type_resource_bounds_with_budget(ty, nodes)?;
        }
        validate_types(&bound.spec_args, &bound_scope, false)?;
    }
    Ok(())
}

fn validate_trace_selector_declarations(
    environment: &ProjectEnvironmentInput,
    std_registry: &etas_std::StdRegistry,
) -> Result<(), ExternalEnvironmentValidationError> {
    let bounds = || {
        environment
            .external_public_metadata
            .iter()
            .flat_map(|metadata| {
                metadata
                    .trace_spec_summaries
                    .iter()
                    .flat_map(move |summary| {
                        summary
                            .alternatives
                            .iter()
                            .flatten()
                            .flat_map(move |clause| {
                                clause
                                    .selector_bounds
                                    .iter()
                                    .map(move |bound| (metadata.package, bound))
                            })
                    })
            })
    };
    if bounds().next().is_none() {
        return Ok(());
    }
    let standard = etas_effects::EffectRegistry::with_standard_effects_from(std_registry);
    let mut count = 0usize;
    let mut specs = BTreeMap::new();
    let mut actions = BTreeMap::new();
    let mut dependencies = BTreeSet::new();
    for edge in &environment.external_package_dependencies {
        count = count.checked_add(1).ok_or_else(|| {
            ExternalEnvironmentValidationError::package(
                edge.package,
                "trace selector dependency count overflow",
            )
        })?;
        if count > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err(ExternalEnvironmentValidationError::package(
                edge.package,
                "trace selector dependencies exceed maximum node count",
            ));
        }
        dependencies.insert((edge.package, edge.dependency));
    }
    for metadata in &environment.external_public_metadata {
        for spec in &metadata.spec_signatures {
            count = count.checked_add(1).ok_or_else(|| {
                ExternalEnvironmentValidationError::package(
                    metadata.package,
                    "trace selector declaration count overflow",
                )
            })?;
            if count > MAX_EXTERNAL_METADATA_GRAPH_NODES {
                return Err(ExternalEnvironmentValidationError::package(
                    metadata.package,
                    "trace selector declarations exceed maximum node count",
                ));
            }
            specs
                .entry(spec.path.as_slice())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((metadata.package, spec)));
        }
        for action in &metadata.actions {
            count = count.checked_add(1).ok_or_else(|| {
                ExternalEnvironmentValidationError::package(
                    metadata.package,
                    "trace selector declaration count overflow",
                )
            })?;
            if count > MAX_EXTERNAL_METADATA_GRAPH_NODES {
                return Err(ExternalEnvironmentValidationError::package(
                    metadata.package,
                    "trace selector declarations exceed maximum node count",
                ));
            }
            actions
                .entry(action.path.as_slice())
                .and_modify(|entry| *entry = None)
                .or_insert(Some((metadata.package, action)));
        }
    }
    for (package, bound) in bounds() {
        count = count.checked_add(1).ok_or_else(|| {
            ExternalEnvironmentValidationError::package(
                package,
                "trace selector bound count overflow",
            )
        })?;
        if count > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                "trace selector bounds exceed maximum node count",
            ));
        }
        let external_spec = specs.get(bound.spec_path.as_slice());
        let std_spec = std_registry
            .lookup_qualified(&bound.spec_path)
            .or_else(|| {
                (bound.spec_path.len() == 1)
                    .then(|| std_registry.lookup_prelude(&bound.spec_path[0]))
                    .flatten()
            })
            .and_then(|symbol| match &symbol.decl {
                etas_std::StdDecl::Type(decl) if decl.kind == etas_std::TypeDeclKind::Spec => {
                    Some(decl.params.len())
                }
                _ => None,
            });
        let spec_arity = match (external_spec, std_spec) {
            (Some(Some((owner, spec))), None)
                if spec.kind == crate::ProjectExternalSpecKindInput::Type =>
            {
                validate_trace_selector_owner(
                    &dependencies,
                    package,
                    *owner,
                    &spec.visibility,
                    "spec",
                )?;
                spec.param_names.len()
            }
            (None, Some(arity)) => arity,
            _ => {
                return Err(ExternalEnvironmentValidationError::package(
                    package,
                    format!(
                        "trace selector spec `{}` is missing, ambiguous, or not a type spec",
                        bound.spec_path.join(".")
                    ),
                ));
            }
        };
        if spec_arity != bound.spec_args.len() {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                format!(
                    "trace selector spec `{}` expects {spec_arity} argument(s), got {}",
                    bound.spec_path.join("."),
                    bound.spec_args.len()
                ),
            ));
        }
        let external_action = actions.get(bound.effect_path.as_slice());
        let standard_action = standard.standard_action_by_name(&bound.effect_path.join("."));
        let index = usize::try_from(bound.argument).map_err(|_| {
            ExternalEnvironmentValidationError::package(
                package,
                "trace selector argument index is invalid",
            )
        })?;
        let is_type_selector = match (external_action, standard_action) {
            (Some(Some((owner, action))), None) => {
                validate_trace_selector_owner(
                    &dependencies,
                    package,
                    *owner,
                    &action.visibility,
                    "action",
                )?;
                matches!(
                    action.effect_args.get(index),
                    Some(crate::ProjectExternalActionArgKindInput::Type)
                )
            }
            (None, Some(action)) => standard.action_signature(&action).is_some_and(|signature| {
                matches!(
                    signature.effect_args.get(index),
                    Some(etas_effects::EffectActionArgKind::Type)
                )
            }),
            _ => false,
        };
        if !is_type_selector {
            return Err(ExternalEnvironmentValidationError::package(
                package,
                format!(
                    "trace selector `{}` must reference a unique checked type selector",
                    bound.effect_path.join(".")
                ),
            ));
        }
    }
    Ok(())
}

fn validate_trace_selector_owner(
    dependencies: &BTreeSet<(ExternalPackageId, ExternalPackageId)>,
    package: ExternalPackageId,
    owner: ExternalPackageId,
    visibility: &str,
    kind: &str,
) -> Result<(), ExternalEnvironmentValidationError> {
    if owner == package {
        return Ok(());
    }
    if visibility == "public" && dependencies.contains(&(package, owner)) {
        return Ok(());
    }
    Err(ExternalEnvironmentValidationError::package(
        package,
        format!("trace selector {kind} must belong to the package or a declared public dependency"),
    ))
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
        if signature.param_names.len() != signature.generic_params.len()
            || signature
                .param_names
                .iter()
                .zip(&signature.generic_params)
                .any(|(name, param)| name != &param.name)
        {
            return Err("spec generic parameter facts are incomplete or inconsistent".into());
        }
        let scope = generic_scope(&signature.path, &signature.generic_params)?;
        for param in &signature.generic_params {
            for bound in &param.bounds {
                validate_path(&bound.spec, "spec generic bound path")?;
                validate_types(&bound.args, &scope, false)?;
            }
        }
        for bound in &signature.super_specs {
            validate_path(&bound.spec, "super spec path")?;
            validate_types(&bound.args, &scope, false)?;
        }
        if let Some(callable) = &signature.callable {
            validate_callable_in_scope(
                &callable.path,
                &callable.generic_params,
                &callable.params,
                &callable.output,
                callable.effects.as_ref(),
                &scope,
            )?;
        }
        for method in &signature.methods {
            validate_path(&method.path, "spec method path")?;
            if method.name.is_empty() {
                return Err("spec method name must not be empty".to_owned());
            }
            if let Some(index) = method.receiver_param {
                let callable = method
                    .signature
                    .as_ref()
                    .ok_or("spec receiver requires a callable signature")?;
                let parameter = callable
                    .generic_params
                    .get(index as usize)
                    .ok_or("spec receiver parameter index is out of bounds")?;
                if parameter.kind != crate::ProjectExternalCallableGenericParamKindInput::Type {
                    return Err("spec receiver parameter must have type kind".into());
                }
                if !matches!(callable.params.first(), Some(crate::ProjectExternalTypeInput::Var(name)) if name == &parameter.name)
                {
                    return Err("spec receiver must bind the first callable input type".into());
                }
            }
            if let Some(callable) = &method.signature {
                validate_callable_in_scope(
                    &callable.path,
                    &callable.generic_params,
                    &callable.params,
                    &callable.output,
                    callable.effects.as_ref(),
                    &scope,
                )?;
            }
        }
    }
    let mut methods = std::collections::BTreeMap::new();
    for method in &metadata.methods {
        if methods.insert(&method.identity, method).is_some() {
            return Err("published method identity is duplicated".into());
        }
        method.validate().map_err(|error| error.to_string())?;
        etas_package_metadata::validate_callable_value_graph(&method.callable)
            .map_err(|error| error.to_string())?;
    }
    for implementation in &metadata.spec_impls {
        validate_path(&implementation.spec, "spec implementation path")?;
        validate_type(&implementation.self_type, &GenericScope::default(), true)?;
        validate_types(&implementation.args, &GenericScope::default(), true)?;
        let mut identities = BTreeSet::new();
        for identity in &implementation.methods {
            if !identities.insert(identity) {
                return Err("spec implementation contains duplicate method identities".into());
            }
            if !methods.contains_key(identity) {
                return Err("spec implementation refers to an unpublished method".into());
            }
        }
    }
    for satisfaction in &metadata.type_spec_satisfactions {
        validate_path(&satisfaction.spec, "type satisfaction spec path")?;
        validate_type(&satisfaction.self_type, &GenericScope::default(), true)?;
        validate_types(&satisfaction.args, &GenericScope::default(), true)?;
    }
    for satisfaction in &metadata.callable_spec_satisfactions {
        validate_contract_target(&satisfaction.item, &methods)?;
        validate_path(&satisfaction.spec, "callable satisfaction spec path")?;
        validate_types(&satisfaction.args, &GenericScope::default(), true)?;
    }
    for conformance in &metadata.trace_spec_conformances {
        validate_contract_target(&conformance.item, &methods)?;
        if let crate::ProjectExternalTraceSpecConformanceTargetInput::Named { spec, args, .. } =
            &conformance.target
        {
            validate_path(spec, "trace conformance spec path")?;
            validate_types(args, &GenericScope::default(), true)?;
        }
    }
    Ok(())
}

fn validate_contract_target(
    target: &etas_package_metadata::CallableContractTarget,
    methods: &std::collections::BTreeMap<
        &etas_code::MethodId,
        &etas_package_metadata::PublishedMethod,
    >,
) -> Result<(), String> {
    match target {
        etas_package_metadata::CallableContractTarget::Item(path) => {
            validate_path(path, "callable conformance path")
        }
        etas_package_metadata::CallableContractTarget::Method(identity) => {
            if !methods.contains_key(identity) {
                return Err(
                    "callable conformance method requires one published implementation owner"
                        .into(),
                );
            }
            Ok(())
        }
    }
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

fn callable_definition_argument<'a>(
    ty: &'a ProjectExternalTypeInput,
    std_registry: &etas_std::StdRegistry,
) -> Option<&'a ProjectExternalTypeInput> {
    let mut ty = ty;
    while let ProjectExternalTypeInput::Alias { target, .. } = ty {
        ty = target;
    }
    let ProjectExternalTypeInput::Applied { path, args } = ty else {
        return None;
    };
    let symbol = std_registry.lookup_qualified(path)?;
    if !matches!(&symbol.decl, etas_std::StdDecl::Type(decl)
        if decl.code_role == Some(etas_std::CodeTypeRole::CallableDefinition))
    {
        return None;
    }
    let [argument] = args.as_slice() else {
        return None;
    };
    Some(argument)
}

fn trace_callable_parameter(
    ty: &ProjectExternalTypeInput,
    parameters: &[ProjectExternalTypeInput],
    scope: &GenericScope,
    std_registry: &etas_std::StdRegistry,
) -> bool {
    let mut ty = ty;
    while let ProjectExternalTypeInput::Alias { target, .. } = ty {
        ty = target;
    }
    match ty {
        ProjectExternalTypeInput::Function { .. } => true,
        ProjectExternalTypeInput::Var(name) if scope.types.contains(name) => parameters.iter().any(|parameter| {
            matches!(callable_definition_argument(parameter, std_registry), Some(ProjectExternalTypeInput::Var(other)) if name == other)
        }),
        _ => match callable_definition_argument(ty, std_registry) {
            Some(ProjectExternalTypeInput::Function { .. }) => true,
            Some(ProjectExternalTypeInput::Var(name)) => scope.types.contains(name),
            _ => false,
        },
    }
}

fn validate_effect_summaries(
    metadata: &ProjectExternalPublicMetadataInput,
    callable_scopes: &HashMap<Vec<String>, GenericScope>,
    std_registry: &etas_std::StdRegistry,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for summary in &metadata.effect_summaries {
        if !summary.residual_checks.is_empty()
            || summary
                .latent_flows
                .iter()
                .any(|latent| !latent.residual_checks.is_empty())
        {
            return Err(
                "external effect residual references require validated residual plans".into(),
            );
        }
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
        if let Some(defaults) = &summary.default_handler_effects {
            validate_effect_row(&defaults.direct_effects, scope)?;
            validate_effect_row(&defaults.default_actions, scope)?;
            let mut actions = HashSet::new();
            for handler in &defaults.handlers {
                validate_effect_ref(&handler.action, scope)?;
                validate_effect_row(&handler.produced, scope)?;
                if !actions.insert(&handler.action) {
                    return Err("duplicate default handler contribution".into());
                }
            }
        }
        for latent in &summary.latent_flows {
            validate_effect_row(&latent.declared_bound, scope)?;
            validate_effect_row(&latent.inferred_effects, scope)?;
        }
        let (parameter_names, parameter_types) = metadata
            .flows
            .iter()
            .find(|callable| callable.path == summary.item)
            .map(|callable| (callable.param_names.as_slice(), callable.params.as_slice()))
            .or_else(|| {
                metadata
                    .agents
                    .iter()
                    .find(|callable| callable.path == summary.item)
                    .map(|callable| (callable.param_names.as_slice(), callable.input.as_slice()))
            })
            .or_else(|| {
                metadata
                    .tools
                    .iter()
                    .find(|callable| callable.path == summary.item)
                    .map(|callable| (callable.param_names.as_slice(), callable.input.as_slice()))
            })
            .unwrap_or_default();
        let callable_parameters = parameter_names
            .iter()
            .zip(parameter_types)
            .filter_map(|(name, ty)| {
                trace_callable_parameter(ty, parameter_types, scope, std_registry)
                    .then_some(name.clone())
            })
            .collect::<Vec<_>>();
        validate_action_trace(
            &summary.action_trace,
            scope,
            &callable_parameters,
            parameter_names.len(),
        )?;
        if let Some(behavior) = &summary.behavior {
            if behavior
                .independent_determinism
                .is_some_and(|value| value > behavior.determinism)
            {
                return Err("external independent determinism exceeds its summary".into());
            }
            let mut traces = vec![&summary.action_trace];
            let mut parameter_calls = false;
            while let Some(trace) = traces.pop() {
                match trace {
                    ProjectExternalActionTraceInput::ParameterCall { .. } => parameter_calls = true,
                    ProjectExternalActionTraceInput::Seq(parts)
                    | ProjectExternalActionTraceInput::Choice(parts) => traces.extend(parts),
                    ProjectExternalActionTraceInput::Repeat(part) => traces.push(part),
                    ProjectExternalActionTraceInput::Widened {
                        parameter_calls: calls,
                        ..
                    } => {
                        parameter_calls |= !calls.is_empty();
                    }
                    _ => {}
                }
            }
            if parameter_calls && behavior.independent_determinism.is_none() {
                return Err("external parameter trace is missing independent determinism".into());
            }
        }
        if let Some(requests) = &summary.handler_requests {
            requests::validate(requests, scope, parameter_names)?;
        }
        if let Some(graph) = &summary.returned_callables {
            if graph
                .values
                .iter()
                .any(|node| !node.residual_checks.is_empty())
            {
                return Err(
                    "external callable residual references require validated residual plans".into(),
                );
            }
            if parameter_names.len() != parameter_types.len() {
                return Err(
                    "returned callable evidence requires an unambiguous named parameter interface"
                        .into(),
                );
            }
            etas_package_metadata::validate_callable_value_graph(graph)
                .map_err(|error| error.to_string())?;
            let generics = metadata
                .flows
                .iter()
                .find(|callable| callable.path == summary.item)
                .map(|callable| &callable.generic_params)
                .or_else(|| {
                    metadata
                        .agents
                        .iter()
                        .find(|callable| callable.path == summary.item)
                        .map(|callable| &callable.generic_params)
                })
                .or_else(|| {
                    metadata
                        .tools
                        .iter()
                        .find(|callable| callable.path == summary.item)
                        .map(|callable| &callable.generic_params)
                });
            let generic_params = generics
                .into_iter()
                .flatten()
                .map(|generic| match generic.kind {
                    ProjectExternalCallableGenericParamKindInput::Type => {
                        etas_package_metadata::GenericParamKind::Type
                    }
                    ProjectExternalCallableGenericParamKindInput::Effect => {
                        etas_package_metadata::GenericParamKind::Effect
                    }
                })
                .collect::<Vec<_>>();
            graph
                .validate_references(Some((parameter_types.len(), &generic_params)))
                .map_err(|error| error.to_string())?;
        }
    }
    let required = metadata
        .flows
        .iter()
        .map(|callable| &callable.path)
        .chain(
            metadata
                .agents
                .iter()
                .filter(|agent| agent.output.is_some())
                .map(|callable| &callable.path),
        )
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
    root_parameter_count: usize,
) -> Result<(), String> {
    validate_action_trace_resource_bounds(trace)?;
    validate_action_trace_semantics(trace, scope, parameter_names, root_parameter_count)
}

fn validate_action_trace_semantics(
    trace: &ProjectExternalActionTraceInput,
    scope: &GenericScope,
    parameter_names: &[String],
    root_parameter_count: usize,
) -> Result<(), String> {
    match trace {
        ProjectExternalActionTraceInput::Empty => Ok(()),
        ProjectExternalActionTraceInput::Event { action, .. } => validate_effect_ref(action, scope),
        ProjectExternalActionTraceInput::ParameterCall { parameter } => {
            requests::validate_target(parameter, scope, parameter_names, root_parameter_count)
        }
        ProjectExternalActionTraceInput::Seq(children) => {
            for child in children {
                validate_action_trace_semantics(
                    child,
                    scope,
                    parameter_names,
                    root_parameter_count,
                )?;
            }
            Ok(())
        }
        ProjectExternalActionTraceInput::Choice(children) => {
            if children.is_empty() {
                return Err("external action trace choice requires an alternative".into());
            }
            for child in children {
                validate_action_trace_semantics(
                    child,
                    scope,
                    parameter_names,
                    root_parameter_count,
                )?;
            }
            Ok(())
        }
        ProjectExternalActionTraceInput::Repeat(child) => {
            validate_action_trace_semantics(child, scope, parameter_names, root_parameter_count)
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
                requests::validate_target(parameter, scope, parameter_names, root_parameter_count)?;
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
    validate_callable_in_scope(
        path,
        params,
        inputs,
        output,
        effects,
        &GenericScope::default(),
    )
}

fn validate_callable_in_scope(
    path: &[String],
    params: &[ProjectExternalCallableGenericParamInput],
    inputs: &[ProjectExternalTypeInput],
    output: &ProjectExternalTypeInput,
    effects: Option<&ProjectExternalEffectRowInput>,
    outer: &GenericScope,
) -> Result<GenericScope, String> {
    validate_path(path, "callable path")?;
    let mut scope = generic_scope(path, params)?;
    if scope.types.iter().any(|name| outer.effects.contains(name))
        || scope.effects.iter().any(|name| outer.types.contains(name))
    {
        return Err(
            "nested callable generic parameter changes its enclosing parameter kind".into(),
        );
    }
    scope.types.extend(outer.types.iter().cloned());
    scope.effects.extend(outer.effects.iter().cloned());
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
                scope
                    .parameter_kinds
                    .push(etas_package_metadata::GenericParamKind::Type);
                scope.types.insert(param.name.clone());
            }
            ProjectExternalCallableGenericParamKindInput::Effect => {
                scope
                    .parameter_kinds
                    .push(etas_package_metadata::GenericParamKind::Effect);
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
        ProjectExternalTypeInput::Existential { bounds } => {
            if bounds.is_empty() {
                return Err("existential type requires at least one spec bound".into());
            }
            for bound in bounds {
                validate_path(&bound.spec, "existential spec path")?;
                validate_types_semantics(&bound.args, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
        ProjectExternalTypeInput::Primitive(name) => {
            validate_name(name, "primitive name")?;
            if !etas_types::lower::external::is_valid_external_primitive(name) {
                return Err(format!("unknown primitive type `{name}`"));
            }
            Ok(())
        }
        ProjectExternalTypeInput::Var(name) => {
            validate_name(name, "type variable")?;
            if !allow_unbound_vars && !scope.types.contains(name) {
                return Err(format!(
                    "type variable `{name}` is not declared by the callable"
                ));
            }
            Ok(())
        }
        ProjectExternalTypeInput::Parameter { owner, .. } => {
            if owner.is_portable_parameter_owner() {
                Ok(())
            } else {
                Err("type parameter requires a portable checked owner".into())
            }
        }
        ProjectExternalTypeInput::Named(path) => validate_path(path, "named type path"),
        ProjectExternalTypeInput::Enum { path, variants } => {
            validate_path(path, "enum type path")?;
            let mut names = BTreeSet::new();
            for variant in variants {
                validate_name(&variant.name, "enum variant name")?;
                if !names.insert(&variant.name) {
                    return Err("duplicate enum variant".into());
                }
                if !matches!(
                    variant.ty,
                    ProjectExternalTypeInput::Tuple(_) | ProjectExternalTypeInput::Record { .. }
                ) {
                    return Err("enum variant has invalid payload form".into());
                }
                validate_type_semantics(&variant.ty, scope, allow_unbound_vars)?;
            }
            Ok(())
        }
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
        ProjectExternalTypeInput::Task { output, effects } => {
            validate_type_semantics(output, scope, allow_unbound_vars)?;
            validate_effect_row(effects, scope)
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
    let mut nodes = 0usize;
    validate_type_resource_bounds_with_budget(root, &mut nodes)
}

fn push_type_graph_node<'a>(
    stack: &mut Vec<(TypeGraphNode<'a>, usize)>,
    node: TypeGraphNode<'a>,
    depth: usize,
    visited: usize,
) -> Result<(), String> {
    if stack
        .len()
        .checked_add(visited)
        .and_then(|size| size.checked_add(1))
        .is_none_or(|size| size > MAX_EXTERNAL_METADATA_GRAPH_NODES)
    {
        return Err("external metadata type graph exceeds maximum node count".into());
    }
    stack.push((node, depth));
    Ok(())
}

fn validate_type_resource_bounds_with_budget(
    root: &ProjectExternalTypeInput,
    nodes: &mut usize,
) -> Result<(), String> {
    let mut stack = Vec::new();
    push_type_graph_node(&mut stack, TypeGraphNode::Type(root), 0, *nodes)?;
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_EXTERNAL_METADATA_GRAPH_DEPTH {
            return Err("external metadata type graph exceeds maximum depth".to_owned());
        }
        *nodes = nodes
            .checked_add(1)
            .ok_or_else(|| "external metadata type graph node count overflow".to_owned())?;
        if *nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
            return Err("external metadata type graph exceeds maximum node count".to_owned());
        }
        let next = depth + 1;
        match node {
            TypeGraphNode::Type(ty) => match ty {
                ProjectExternalTypeInput::Existential { bounds } => {
                    for bound in bounds {
                        *nodes = nodes
                            .checked_add(1)
                            .ok_or("external metadata node count overflow")?;
                        if *nodes > MAX_EXTERNAL_METADATA_GRAPH_NODES {
                            return Err(
                                "external metadata type graph exceeds maximum node count".into()
                            );
                        }
                        for arg in &bound.args {
                            push_type_graph_node(
                                &mut stack,
                                TypeGraphNode::Type(arg),
                                next + 1,
                                *nodes,
                            )?;
                        }
                    }
                }
                ProjectExternalTypeInput::Applied { args, .. }
                | ProjectExternalTypeInput::Tuple(args)
                | ProjectExternalTypeInput::ResourceHandle { args, .. } => {
                    for ty in args {
                        push_type_graph_node(&mut stack, TypeGraphNode::Type(ty), next, *nodes)?;
                    }
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
                    push_type_graph_node(&mut stack, TypeGraphNode::Type(target), next, *nodes)?;
                }
                ProjectExternalTypeInput::Nominal { representation, .. } => {
                    if let Some(representation) = representation {
                        push_type_graph_node(
                            &mut stack,
                            TypeGraphNode::Type(representation),
                            next,
                            *nodes,
                        )?;
                    }
                }
                ProjectExternalTypeInput::Map { key, value }
                | ProjectExternalTypeInput::Store { key, value }
                | ProjectExternalTypeInput::Result {
                    ok: key,
                    err: value,
                } => {
                    push_type_graph_node(&mut stack, TypeGraphNode::Type(key), next, *nodes)?;
                    push_type_graph_node(&mut stack, TypeGraphNode::Type(value), next, *nodes)?;
                }
                ProjectExternalTypeInput::Record { fields }
                | ProjectExternalTypeInput::Enum {
                    variants: fields, ..
                } => {
                    for field in fields {
                        push_type_graph_node(
                            &mut stack,
                            TypeGraphNode::Type(&field.ty),
                            next,
                            *nodes,
                        )?;
                    }
                }
                ProjectExternalTypeInput::Function {
                    input,
                    output,
                    effects,
                } => {
                    for ty in input {
                        push_type_graph_node(&mut stack, TypeGraphNode::Type(ty), next, *nodes)?;
                    }
                    push_type_graph_node(&mut stack, TypeGraphNode::Type(output), next, *nodes)?;
                    if let Some(effects) = effects {
                        push_type_graph_node(
                            &mut stack,
                            TypeGraphNode::EffectRow(effects),
                            next,
                            *nodes,
                        )?;
                    }
                }
                ProjectExternalTypeInput::Task { output, effects } => {
                    push_type_graph_node(&mut stack, TypeGraphNode::Type(output), next, *nodes)?;
                    push_type_graph_node(
                        &mut stack,
                        TypeGraphNode::EffectRow(effects),
                        next,
                        *nodes,
                    )?;
                }
                ProjectExternalTypeInput::Handler {
                    handled,
                    produced,
                    result,
                } => {
                    push_type_graph_node(
                        &mut stack,
                        TypeGraphNode::EffectRow(handled),
                        next,
                        *nodes,
                    )?;
                    if let Some(produced) = produced {
                        push_type_graph_node(
                            &mut stack,
                            TypeGraphNode::EffectRow(produced),
                            next,
                            *nodes,
                        )?;
                    }
                    if let Some(result) = result {
                        push_type_graph_node(
                            &mut stack,
                            TypeGraphNode::Type(result),
                            next,
                            *nodes,
                        )?;
                    }
                }
                ProjectExternalTypeInput::Trust { inner, .. } => {
                    push_type_graph_node(&mut stack, TypeGraphNode::Type(inner), next, *nodes)?;
                }
                ProjectExternalTypeInput::Primitive(_)
                | ProjectExternalTypeInput::Var(_)
                | ProjectExternalTypeInput::Parameter { .. }
                | ProjectExternalTypeInput::Named(_)
                | ProjectExternalTypeInput::Prompt
                | ProjectExternalTypeInput::PromptPart => {}
            },
            TypeGraphNode::EffectRow(row) => {
                for effect in &row.effects {
                    push_type_graph_node(
                        &mut stack,
                        TypeGraphNode::EffectRef(effect),
                        next,
                        *nodes,
                    )?;
                }
            }
            TypeGraphNode::EffectRef(effect) => {
                for arg in &effect.args {
                    push_type_graph_node(&mut stack, TypeGraphNode::EffectArg(arg), next, *nodes)?;
                }
            }
            TypeGraphNode::EffectArg(ProjectExternalEffectArgInput::Type(ty)) => {
                push_type_graph_node(&mut stack, TypeGraphNode::Type(ty), next, *nodes)?;
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
