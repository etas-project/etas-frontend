use std::{collections::HashMap, path::PathBuf, sync::Arc};

use etas_core::{Diagnostic, SourceId, id_type};
use etas_effects::{
    DependencyEffectMetadata, EffectFacts, EffectPipelineArtifacts, EffectRegistry,
};
use etas_hir::{HirItemId, HirModuleId, HirProgram, SymbolId, TopLevelLetClassification};
use etas_syntax::ast;
use etas_types::{TypeFacts, TypeId, TypeOutput, TypeStore};
use etas_utils::UnitKindKey;

use crate::project::{
    AffectedModuleSet, HirBodyBindings, HirItemBindings, HirOutput, ImportGraph, ModuleIndex,
    ModuleTopoOrder, ParseOutput, ParsedSource, ResolvedImports, ResolvedPaths, SourceBundle,
    SourceInput, SourceSet, UnitTree,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, etas_core::RetainedSize,
)]
pub enum OptimizationLevel {
    O0,
    O1,
    #[default]
    O2,
    O3,
}

impl OptimizationLevel {
    pub const fn number(self) -> u8 {
        match self {
            Self::O0 => 0,
            Self::O1 => 1,
            Self::O2 => 2,
            Self::O3 => 3,
        }
    }

    pub const fn enables(self, minimum: Self) -> bool {
        self.number() >= minimum.number()
    }
}

impl std::fmt::Display for OptimizationLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "O{}", self.number())
    }
}

impl TryFrom<u8> for OptimizationLevel {
    type Error = OptimizationLevelParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::O0),
            1 => Ok(Self::O1),
            2 => Ok(Self::O2),
            3 => Ok(Self::O3),
            _ => Err(OptimizationLevelParseError),
        }
    }
}

impl std::str::FromStr for OptimizationLevel {
    type Err = OptimizationLevelParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "0" | "O0" | "o0" => Ok(Self::O0),
            "1" | "O1" | "o1" => Ok(Self::O1),
            "2" | "O2" | "o2" => Ok(Self::O2),
            "3" | "O3" | "o3" => Ok(Self::O3),
            _ => Err(OptimizationLevelParseError),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptimizationLevelParseError;

impl std::fmt::Display for OptimizationLevelParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("optimization level must be one of 0, 1, 2, or 3")
    }
}

impl std::error::Error for OptimizationLevelParseError {}

id_type!(ProjectId);
id_type!(ModuleId);
id_type!(ModulePartId);
id_type!(UnitId);
id_type!(ExternalPackageId);
id_type!(ExternalModuleId);
id_type!(ExternalSymbolId);

pub const PROJECT_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "project");
pub const SOURCE_FILE_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "source_file");
pub const MODULE_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "module");
pub const MODULE_PART_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "module_part");
pub const ITEM_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "item");
pub const BODY_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "body");
pub const BLOCK_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "block");
pub const EXPRESSION_UNIT_KIND: UnitKindKey = UnitKindKey::new("frontend", "expression");

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ProjectInput {
    pub project_root: PathBuf,
    pub source_root: Option<PathBuf>,
    pub options: ProjectCompileOptions,
    pub environment: ProjectEnvironmentInput,
    pub sources: Vec<SourceInput>,
    pub entry: ProjectEntry,
}

impl ProjectInput {
    pub fn single_source(source: SourceInput) -> Self {
        Self {
            project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            source_root: None,
            options: ProjectCompileOptions::default(),
            environment: ProjectEnvironmentInput::default(),
            sources: vec![source],
            entry: ProjectEntry {
                module: None,
                flow: "main".to_owned(),
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectCompileOptions {
    pub entry_policy: EntryPolicy,
    pub reflection: crate::code::definition::ReflectionOptions,
    /// Select the optional typed-HIR optimization pipeline. O2 is the balanced default.
    pub optimization_level: OptimizationLevel,
    /// Opt in to the policy-changing P3 context transform. Candidate bindings
    /// retain only their most recent whitespace-delimited units.
    pub quality_context_token_budget: Option<usize>,
    /// Implemented residual-enforcement semantics of the selected execution
    /// target. The default supports no residual plan, so every outstanding
    /// residual obligation is rejected.
    pub residual_target: etas_code::residual::target::ResidualTargetCapabilities,
}

impl ProjectCompileOptions {
    pub fn canonical_options_fingerprint(&self) -> String {
        let entry_policy = match self.entry_policy {
            EntryPolicy::CompileOnly => "compile-only",
            EntryPolicy::Optional => "optional",
            EntryPolicy::Runnable => "runnable",
        };
        let (retained, exported) = self.reflection.canonical_entries();
        let optimization = match self.optimization_level {
            OptimizationLevel::O0 => "optimization-level:o0",
            OptimizationLevel::O1 => "optimization-level:o1",
            OptimizationLevel::O2 => "optimization-level:o2",
            OptimizationLevel::O3 => "optimization-level:o3",
        };
        let mut parts = vec![
            "frontend-options",
            "v6",
            entry_policy,
            optimization,
            "retain-bodies",
        ];
        let quality_context_token_budget;
        if let Some(budget) = self.quality_context_token_budget {
            quality_context_token_budget = budget.to_string();
            parts.push("quality-context-policy:token-budget");
            parts.push(&quality_context_token_budget);
        } else {
            parts.push("quality-context-policy:off");
        }
        parts.extend(retained);
        parts.push("export-bodies");
        parts.extend(exported);
        let residual_target;
        if !self.residual_target.is_none() {
            residual_target = self.residual_target.fingerprint().to_string();
            parts.push("residual-target");
            parts.push(&residual_target);
        }
        crate::artifact::fingerprint_text(&parts).to_string()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub enum EntryPolicy {
    #[default]
    CompileOnly,
    Optional,
    Runnable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectEnvironmentInput {
    /// Caller-supplied pre-analysis identities; SourceId is only a lookup key.
    pub stable_source_keys: Vec<ProjectStableSourceKeyInput>,
    /// Package identity supplied by the source owner; never derived from a
    /// source path or a session-local HIR identity by the frontend.
    pub local_source_package: Option<etas_code::PackageKey>,
    /// Per-file identities for standalone inputs without one project package.
    pub standalone_source_packages: Vec<ProjectStandaloneSourcePackage>,
    /// Locked dependency origins supplied by the filesystem/package owner.
    pub dependency_source_packages: Vec<ProjectDependencySourcePackage>,
    pub external_definition_origins: Vec<ProjectExternalDefinitionOriginsInput>,
    pub external_packages: Vec<ProjectExternalPackageInput>,
    pub external_package_dependencies: Vec<ProjectExternalPackageDependencyInput>,
    pub external_modules: Vec<ProjectExternalModuleInput>,
    pub external_public_metadata: Vec<ProjectExternalPublicMetadataInput>,
    pub external_effect_metadata: DependencyEffectMetadata,
    pub tool_bindings: Vec<ProjectToolBindingInput>,
    pub environment_fingerprint: Option<String>,
    pub external_modules_fingerprint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectStableSourceKeyInput {
    pub source: SourceId,
    pub key: etas_code::StableSourceKey,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectStandaloneSourcePackage {
    pub source: SourceId,
    pub package: etas_code::PackageKey,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectDependencySourcePackage {
    pub package: ExternalPackageId,
    pub import_root: String,
    pub definition_package: etas_code::PackageKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePackageOriginError {
    Missing,
    Ambiguous,
    Conflict,
    UnsupportedSource,
}

pub(crate) fn resolve_stable_source_key(
    entries: &[ProjectStableSourceKeyInput],
    source: SourceId,
) -> Result<etas_code::StableSourceKey, SourcePackageOriginError> {
    let mut matching = entries.iter().filter(|entry| entry.source == source);
    let key = matching
        .next()
        .ok_or(SourcePackageOriginError::Missing)?
        .key;
    if matching.next().is_some() {
        return Err(SourcePackageOriginError::Ambiguous);
    }
    if entries
        .iter()
        .any(|entry| entry.source != source && entry.key == key)
    {
        return Err(SourcePackageOriginError::Conflict);
    }
    Ok(key)
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalDefinitionOriginsInput {
    pub package: ExternalPackageId,
    pub origins: etas_code::identity::PackageDefinitionOrigins,
    pub descriptors: Option<etas_code::descriptor::DefinitionDescriptorSet>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub(crate) struct ValidatedExternalEnvironment {
    environment: ProjectEnvironmentInput,
    fingerprint: String,
}

impl ValidatedExternalEnvironment {
    pub(crate) fn new(environment: ProjectEnvironmentInput) -> Self {
        let fingerprint = environment.canonical_environment_fingerprint();
        Self {
            environment,
            fingerprint,
        }
    }

    pub(crate) fn environment(&self) -> &ProjectEnvironmentInput {
        &self.environment
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl ProjectEnvironmentInput {
    pub fn stable_source_key(
        &self,
        source: SourceId,
    ) -> Result<etas_code::StableSourceKey, SourcePackageOriginError> {
        resolve_stable_source_key(&self.stable_source_keys, source)
    }

    /// SourceId is only a session lookup key. The returned package identity is
    /// supplied by the package/source owner and is independent of that ID.
    pub fn source_package(
        &self,
        source: &SourceInput,
    ) -> Result<etas_code::PackageKey, SourcePackageOriginError> {
        self.source_package_for(source.id, &source.kind)
    }

    pub(crate) fn source_package_for(
        &self,
        source: etas_core::SourceId,
        kind: &crate::SourceKind,
    ) -> Result<etas_code::PackageKey, SourcePackageOriginError> {
        use crate::SourceKind;
        match kind {
            SourceKind::SourceProjectFile | SourceKind::SingleFileInput => {
                let mut matches = self
                    .standalone_source_packages
                    .iter()
                    .filter(|entry| entry.source == source);
                let first = matches.next();
                if matches.next().is_some()
                    || (first.is_some() && self.local_source_package.is_some())
                {
                    return Err(SourcePackageOriginError::Ambiguous);
                }
                self.local_source_package
                    .or_else(|| first.map(|entry| entry.package))
                    .ok_or(SourcePackageOriginError::Missing)
            }
            SourceKind::DependencySourceOverlay {
                package,
                import_root,
            } => {
                let mut packages = self
                    .external_packages
                    .iter()
                    .filter(|entry| entry.id == *package);
                let first = packages.next().ok_or(SourcePackageOriginError::Missing)?;
                if packages.next().is_some() {
                    return Err(SourcePackageOriginError::Ambiguous);
                }
                if first.import_root != *import_root {
                    return Err(SourcePackageOriginError::Missing);
                }
                let mut origins = self
                    .dependency_source_packages
                    .iter()
                    .filter(|entry| entry.package == *package);
                let first = origins.next().ok_or(SourcePackageOriginError::Missing)?;
                if origins.next().is_some() {
                    return Err(SourcePackageOriginError::Ambiguous);
                }
                if first.import_root != *import_root {
                    return Err(SourcePackageOriginError::Conflict);
                }
                let mut metadata = self
                    .external_definition_origins
                    .iter()
                    .filter(|entry| entry.package == *package);
                if let Some(origin) = metadata.next() {
                    if metadata.next().is_some() {
                        return Err(SourcePackageOriginError::Ambiguous);
                    }
                    if origin.origins.package != first.definition_package {
                        return Err(SourcePackageOriginError::Conflict);
                    }
                }
                Ok(first.definition_package)
            }
            SourceKind::VirtualStd { .. }
            | SourceKind::VirtualGenerated
            | SourceKind::LspOverlay => Err(SourcePackageOriginError::UnsupportedSource),
        }
    }

    pub fn canonical_environment_fingerprint(&self) -> String {
        self.environment_fingerprint_for(None)
    }

    fn environment_fingerprint_for(&self, selected: Option<ExternalPackageId>) -> String {
        let mut parts = vec!["frontend_project_environment:v10".to_owned()];
        let mut stable_sources = self
            .stable_source_keys
            .iter()
            .filter(|_| selected.is_none())
            .collect::<Vec<_>>();
        stable_sources.sort_by_key(|entry| (entry.source.0, entry.key));
        for entry in stable_sources {
            parts.push(format!(
                "stable-source:{}:{}",
                entry.source.0,
                entry.key.fingerprint()
            ));
        }
        if selected.is_none()
            && let Some(package) = self.local_source_package
        {
            parts.push(format!("local-source-package:{}", package.fingerprint()));
        }
        let mut source_packages = self
            .standalone_source_packages
            .iter()
            .filter(|_| selected.is_none())
            .collect::<Vec<_>>();
        source_packages.sort_by_key(|entry| (entry.source.0, entry.package.fingerprint()));
        for entry in source_packages {
            parts.push(format!(
                "standalone-source-package:{}:{}",
                entry.source.0,
                entry.package.fingerprint()
            ));
        }
        let mut dependency_sources: Vec<_> = self
            .dependency_source_packages
            .iter()
            .filter(|entry| selected.is_none_or(|id| entry.package == id))
            .collect();
        dependency_sources
            .sort_by_key(|entry| (entry.package, &entry.import_root, entry.definition_package));
        for entry in dependency_sources {
            parts.push(format!(
                "dependency-source-package:{}",
                crate::artifact::dependency_source_package(entry)
            ));
        }
        let mut origins: Vec<_> = self
            .external_definition_origins
            .iter()
            .filter(|input| selected.is_none_or(|id| input.package == id))
            .collect();
        origins.sort_by_key(|input| input.package);
        for input in origins {
            parts.push(format!(
                "definition-origins:{}:{}",
                input.package.0,
                input.origins.fingerprint()
            ));
            if let Some(descriptors) = &input.descriptors {
                parts.push(format!(
                    "definition-descriptors:{}:{}",
                    input.package.0,
                    etas_package_metadata::definition_descriptors_fingerprint(descriptors)
                ));
            }
        }
        if let Some(fingerprint) = &self.environment_fingerprint {
            parts.push(format!("external-fingerprint:{fingerprint}"));
        }
        parts.push(format!(
            "external-packages:{}",
            self.external_packages_fingerprint_for(selected)
        ));
        let mut dependencies = self
            .external_package_dependencies
            .iter()
            .filter(|edge| selected.is_none_or(|id| edge.package == id))
            .map(|edge| (edge.package, edge.dependency))
            .collect::<Vec<_>>();
        dependencies.sort();
        for (package, dependency) in dependencies {
            parts.push(format!(
                "external-dependency:{}:{}",
                package.0, dependency.0
            ));
        }
        parts.push(format!(
            "external-modules:{}",
            self.external_modules_fingerprint_for(selected)
        ));
        parts.push("public-metadata".to_owned());
        let mut public_metadata: Vec<_> = self
            .external_public_metadata
            .iter()
            .filter(|metadata| selected.is_none_or(|id| metadata.package == id))
            .collect();
        public_metadata.sort_by(|left, right| left.package.cmp(&right.package));
        for metadata in public_metadata {
            parts.push(format!("public-package:{}", metadata.package.0));
            for signature in &metadata.types {
                parts.push(format!(
                    "type:{}",
                    crate::artifact::external_named_signature(signature)
                ));
            }
            for signature in &metadata.values {
                parts.push(format!(
                    "value:{}",
                    crate::artifact::external_named_signature(signature)
                ));
            }
            for signature in &metadata.enums {
                parts.push(format!(
                    "enum:{}",
                    crate::artifact::external_named_signature(signature)
                ));
            }
            for signature in &metadata.effects {
                parts.push(format!(
                    "effect:{}",
                    crate::artifact::external_named_signature(signature)
                ));
            }
            for signature in &metadata.trace_specs {
                parts.push(format!(
                    "trace-spec:{}",
                    crate::artifact::external_named_signature(signature)
                ));
            }
            for signature in &metadata.flows {
                parts.push(format!(
                    "flow:{}",
                    crate::artifact::external_flow(signature)
                ));
            }
            for signature in &metadata.agents {
                parts.push(format!(
                    "agent:{}",
                    crate::artifact::external_agent(signature)
                ));
            }
            for signature in &metadata.tools {
                parts.push(format!(
                    "tool:{}",
                    crate::artifact::external_tool(signature)
                ));
            }
            for schema in &metadata.tool_schemas {
                parts.push(format!(
                    "tool-schema:{}",
                    crate::artifact::external_tool_schema(schema)
                ));
            }
            for signature in &metadata.actions {
                parts.push(format!(
                    "action:{}",
                    crate::artifact::external_action(signature)
                ));
            }
            for signature in &metadata.spec_signatures {
                parts.push(format!(
                    "spec:{}",
                    crate::artifact::external_spec(signature)
                ));
            }
            for implementation in &metadata.spec_impls {
                parts.push(format!(
                    "spec-impl:{}",
                    crate::artifact::external_spec_impl(implementation)
                ));
            }
            for method in &metadata.methods {
                parts.push(format!(
                    "method:{}",
                    etas_package_metadata::published_method_input_fingerprint(method)
                ));
            }
            for fact in &metadata.type_spec_satisfactions {
                parts.push(format!(
                    "type-spec-satisfaction:{}",
                    crate::artifact::external_type_satisfaction(fact)
                ));
            }
            for fact in &metadata.callable_spec_satisfactions {
                parts.push(format!(
                    "callable-spec-satisfaction:{}",
                    crate::artifact::external_callable_satisfaction(fact)
                ));
            }
            for fact in &metadata.trace_spec_conformances {
                parts.push(format!(
                    "trace-spec-conformance:{}",
                    crate::artifact::external_trace_conformance(fact)
                ));
            }
            for summary in &metadata.effect_summaries {
                parts.push(format!(
                    "effect-summary:{}",
                    crate::artifact::external_effect_summary(summary)
                ));
                match &summary.returned_callables {
                    Some(graph) => parts.push(format!(
                        "returned-callables:some:{}",
                        etas_package_metadata::callable_value_graph_input_fingerprint(graph)
                    )),
                    None => parts.push("returned-callables:none".to_owned()),
                }
            }
            for summary in &metadata.action_summaries {
                parts.push(format!(
                    "action-summary:{}",
                    crate::artifact::external_action_summary(summary)
                ));
            }
            for summary in &metadata.trace_spec_summaries {
                parts.push(format!(
                    "trace-spec-summary:{}",
                    crate::artifact::external_trace_summary(summary)
                ));
            }
            for re_export in &metadata.re_exports {
                parts.push(format!(
                    "re-export:{}",
                    crate::artifact::external_re_export(re_export)
                ));
            }
        }
        parts.push(format!(
            "dependency-effects:{}",
            crate::artifact::external_dependency_effects(&self.external_effect_metadata)
        ));
        parts.push("tool-bindings".to_owned());
        for binding in &self.tool_bindings {
            parts.push(format!(
                "tool:{}",
                crate::artifact::external_tool_binding(binding)
            ));
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        crate::artifact::fingerprint_text(&refs).to_string()
    }

    pub fn canonical_dependency_metadata_fingerprints(&self) -> Vec<(String, String)> {
        let mut packages: Vec<_> = self.external_packages.iter().collect();
        packages.sort_by(|left, right| left.import_root.cmp(&right.import_root));
        packages
            .into_iter()
            .map(|package| {
                let fingerprint = self.environment_fingerprint_for(Some(package.id));
                (package.import_root.clone(), fingerprint)
            })
            .collect()
    }

    pub fn canonical_external_packages_fingerprint(&self) -> String {
        self.external_packages_fingerprint_for(None)
    }

    fn external_packages_fingerprint_for(&self, selected: Option<ExternalPackageId>) -> String {
        let mut parts = vec!["frontend_external_packages:v2".to_owned()];
        let mut packages: Vec<_> = self
            .external_packages
            .iter()
            .filter(|package| selected.is_none_or(|id| package.id == id))
            .collect();
        packages.sort_by(|left, right| {
            left.import_root
                .cmp(&right.import_root)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.version.cmp(&right.version))
                .then_with(|| left.edition.cmp(&right.edition))
                .then_with(|| left.id.cmp(&right.id))
        });
        for package in packages {
            parts.push(crate::artifact::external_package(package));
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        crate::artifact::fingerprint_text(&refs).to_string()
    }

    pub fn canonical_external_modules_fingerprint(&self) -> String {
        self.external_modules_fingerprint_for(None)
    }

    fn external_modules_fingerprint_for(&self, selected: Option<ExternalPackageId>) -> String {
        let mut parts = vec!["frontend_external_modules:v2".to_owned()];
        if let Some(fingerprint) = &self.external_modules_fingerprint {
            parts.push(format!("external-fingerprint:{fingerprint}"));
        }
        let mut modules: Vec<_> = self
            .external_modules
            .iter()
            .filter(|module| selected.is_none_or(|id| module.package == Some(id)))
            .collect();
        modules.sort_by(|left, right| {
            left.path
                .segments
                .cmp(&right.path.segments)
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.package.cmp(&right.package))
        });
        for module in modules {
            parts.push(crate::artifact::external_module(module));
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        crate::artifact::fingerprint_text(&refs).to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalPackageInput {
    pub id: ExternalPackageId,
    pub name: String,
    pub version: String,
    pub edition: String,
    pub import_root: String,
    pub std_compatibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalPackageDependencyInput {
    pub package: ExternalPackageId,
    pub dependency: ExternalPackageId,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalModuleInput {
    pub package: Option<ExternalPackageId>,
    pub id: ExternalModuleId,
    pub path: ModulePath,
    pub exports: Vec<ProjectExternalExportInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalPublicMetadataInput {
    pub package: ExternalPackageId,
    pub types: Vec<ProjectExternalNamedSignatureInput>,
    pub values: Vec<ProjectExternalNamedSignatureInput>,
    pub enums: Vec<ProjectExternalNamedSignatureInput>,
    pub flows: Vec<ProjectExternalFlowSignatureInput>,
    pub agents: Vec<ProjectExternalAgentSignatureInput>,
    pub tools: Vec<ProjectExternalToolSignatureInput>,
    pub tool_schemas: Vec<ProjectExternalToolSchemaInput>,
    pub effects: Vec<ProjectExternalNamedSignatureInput>,
    pub actions: Vec<ProjectExternalActionSignatureInput>,
    pub trace_specs: Vec<ProjectExternalNamedSignatureInput>,
    pub spec_signatures: Vec<ProjectExternalSpecSignatureInput>,
    pub spec_impls: Vec<ProjectExternalSpecImplInput>,
    pub methods: Vec<etas_package_metadata::PublishedMethod>,
    pub type_spec_satisfactions: Vec<ProjectExternalTypeSpecSatisfactionInput>,
    pub callable_spec_satisfactions: Vec<ProjectExternalCallableSpecSatisfactionInput>,
    pub trace_spec_conformances: Vec<ProjectExternalTraceSpecConformanceInput>,
    pub effect_summaries: Vec<ProjectExternalEffectSummaryInput>,
    pub action_summaries: Vec<ProjectExternalActionSummaryInput>,
    pub trace_spec_summaries: Vec<ProjectExternalTraceSpecSummaryInput>,
    pub re_exports: Vec<ProjectExternalReExportInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalNamedSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ProjectExternalCallableGenericParamInput>,
    pub visibility: String,
    pub ty: Option<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalSpecSignatureInput {
    pub path: Vec<String>,
    pub visibility: String,
    pub kind: ProjectExternalSpecKindInput,
    pub param_names: Vec<String>,
    pub generic_params: Vec<ProjectExternalCallableGenericParamInput>,
    pub callable: Option<ProjectExternalFlowSignatureInput>,
    pub methods: Vec<ProjectExternalSpecMethodInput>,
    pub super_specs: Vec<ProjectExternalSpecBoundInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalSpecKindInput {
    Type,
    Callable,
    Trace,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalSpecMethodInput {
    pub name: String,
    pub path: Vec<String>,
    pub signature: Option<ProjectExternalFlowSignatureInput>,
    pub receiver_param: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub struct ProjectExternalSpecBoundInput {
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalSpecImplInput {
    pub self_type: ProjectExternalTypeInput,
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
    pub methods: Vec<etas_code::MethodId>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalTypeSpecSatisfactionInput {
    pub self_type: ProjectExternalTypeInput,
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalCallableSpecSatisfactionInput {
    pub item: etas_package_metadata::CallableContractTarget,
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalTraceSpecConformanceInput {
    pub item: etas_package_metadata::CallableContractTarget,
    pub target: ProjectExternalTraceSpecConformanceTargetInput,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalTraceSpecConformanceTargetInput {
    Inline,
    Named {
        spec: Vec<String>,
        definition: etas_code::DefinitionKey,
        args: Vec<ProjectExternalTypeInput>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalFlowSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ProjectExternalCallableGenericParamInput>,
    pub param_names: Vec<String>,
    pub params: Vec<ProjectExternalTypeInput>,
    pub output: ProjectExternalTypeInput,
    pub effects: Option<ProjectExternalEffectRowInput>,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalAgentSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ProjectExternalCallableGenericParamInput>,
    pub param_names: Vec<String>,
    pub input: Vec<ProjectExternalTypeInput>,
    pub output: Option<ProjectExternalTypeInput>,
    pub effects: Option<ProjectExternalEffectRowInput>,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalToolSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ProjectExternalCallableGenericParamInput>,
    pub param_names: Vec<String>,
    pub input: Vec<ProjectExternalTypeInput>,
    pub output: ProjectExternalTypeInput,
    pub effects: Option<ProjectExternalEffectRowInput>,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalToolSchemaInput {
    pub path: Vec<String>,
    pub schema_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalActionSignatureInput {
    pub path: Vec<String>,
    pub generic_params: Vec<ProjectExternalActionGenericParamInput>,
    pub params: Vec<ProjectExternalTypeInput>,
    pub effect_args: Vec<ProjectExternalActionArgKindInput>,
    pub selector_param_names: Vec<String>,
    pub selector_defaults: Vec<Option<ProjectExternalEffectArgInput>>,
    pub output: ProjectExternalTypeInput,
    pub returns_never: bool,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalCallableGenericParamInput {
    pub name: String,
    pub kind: ProjectExternalCallableGenericParamKindInput,
    pub bounds: Vec<ProjectExternalSpecBoundInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalCallableGenericParamKindInput {
    Type,
    Effect,
}

pub type ProjectExternalActionGenericParamInput = ProjectExternalCallableGenericParamInput;

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalActionArgKindInput {
    Type,
    MemoryPlace,
    StaticResourcePath { ty: String },
    StringPattern,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub struct ProjectExternalEffectRowInput {
    pub effects: Vec<ProjectExternalEffectRefInput>,
    pub tail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub struct ProjectExternalEffectRefInput {
    pub path: Vec<String>,
    pub args: Vec<ProjectExternalEffectArgInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub enum ProjectExternalEffectArgInput {
    Type(ProjectExternalTypeInput),
    Path(Vec<String>),
    String(String),
    Int(String),
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub enum ProjectExternalTypeInput {
    AgentContext(etas_code::MethodId),
    Existential {
        bounds: Vec<ProjectExternalSpecBoundInput>,
    },
    Primitive(String),
    Var(String),
    Parameter {
        owner: etas_code::descriptor::GenericOwner,
        position: u32,
    },
    Named(Vec<String>),
    Enum {
        path: Vec<String>,
        variants: Vec<ProjectExternalRecordFieldInput>,
    },
    Applied {
        path: Vec<String>,
        args: Vec<ProjectExternalTypeInput>,
    },
    Alias {
        path: Vec<String>,
        target: Box<ProjectExternalTypeInput>,
    },
    Nominal {
        path: Vec<String>,
        representation: Option<Box<ProjectExternalTypeInput>>,
    },
    Array(Box<ProjectExternalTypeInput>),
    List(Box<ProjectExternalTypeInput>),
    Map {
        key: Box<ProjectExternalTypeInput>,
        value: Box<ProjectExternalTypeInput>,
    },
    Set(Box<ProjectExternalTypeInput>),
    Range(Box<ProjectExternalTypeInput>),
    Slice(Box<ProjectExternalTypeInput>),
    Option(Box<ProjectExternalTypeInput>),
    Result {
        ok: Box<ProjectExternalTypeInput>,
        err: Box<ProjectExternalTypeInput>,
    },
    Record {
        fields: Vec<ProjectExternalRecordFieldInput>,
    },
    Tuple(Vec<ProjectExternalTypeInput>),
    Function {
        input: Vec<ProjectExternalTypeInput>,
        output: Box<ProjectExternalTypeInput>,
        effects: Option<ProjectExternalEffectRowInput>,
    },
    Task {
        output: Box<ProjectExternalTypeInput>,
        effects: ProjectExternalEffectRowInput,
    },
    Handler {
        handled: ProjectExternalEffectRowInput,
        produced: Option<ProjectExternalEffectRowInput>,
        result: Option<Box<ProjectExternalTypeInput>>,
    },
    Trust {
        wrapper: String,
        inner: Box<ProjectExternalTypeInput>,
    },
    Prompt,
    PromptPart,
    Message(Box<ProjectExternalTypeInput>),
    MemorySelection(Box<ProjectExternalTypeInput>),
    Store {
        key: Box<ProjectExternalTypeInput>,
        value: Box<ProjectExternalTypeInput>,
    },
    MemoryRegion(Box<ProjectExternalTypeInput>),
    ResourceHandle {
        name: String,
        args: Vec<ProjectExternalTypeInput>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub struct ProjectExternalRecordFieldInput {
    pub name: String,
    pub ty: ProjectExternalTypeInput,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalEffectSummaryInput {
    pub residual_checks: Vec<etas_code::residual::identity::ResidualObligationRef>,
    pub behavior: Option<etas_package_metadata::CallableValueBehavior>,
    pub handler_requests: Option<ProjectExternalRequestInstantiationInput>,
    pub returned_callables: Option<etas_package_metadata::CallableValueGraph>,
    pub default_handler_effects: Option<ProjectExternalDefaultHandlerEffectsInput>,
    pub item: Vec<String>,
    pub public_effects: ProjectExternalEffectRowInput,
    pub requested_actions: ProjectExternalEffectRowInput,
    pub handled_requested_actions: ProjectExternalEffectRowInput,
    pub latent_flows: Vec<ProjectExternalLatentFlowSummaryInput>,
    pub action_trace: ProjectExternalActionTraceInput,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalRequestInstantiationInput {
    pub actions: Vec<ProjectExternalActionRequestInput>,
    pub parameters: Vec<ProjectExternalParameterRequestInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalActionRequestInput {
    pub action: ProjectExternalEffectRefInput,
    pub type_bindings: Option<Vec<(String, ProjectExternalTypeInput)>>,
    pub signature: Option<ProjectExternalActionRequestSignatureInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalActionRequestSignatureInput {
    pub params: Vec<ProjectExternalTypeInput>,
    pub output: ProjectExternalTypeInput,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalParameterRequestInput {
    pub parameter: etas_package_metadata::CallableRequestTarget,
    pub excluded: Vec<ProjectExternalEffectRefInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalDefaultHandlerEffectsInput {
    pub behavior: Option<etas_package_metadata::DefaultBehavior>,
    pub direct_effects: ProjectExternalEffectRowInput,
    pub default_actions: ProjectExternalEffectRowInput,
    pub handlers: Vec<ProjectExternalDefaultHandlerContributionInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalDefaultHandlerContributionInput {
    pub action: ProjectExternalEffectRefInput,
    pub produced: ProjectExternalEffectRowInput,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalActionTraceInput {
    #[default]
    Empty,
    DefaultHandler {
        guards: Vec<ProjectExternalEffectRefInput>,
        body: Box<ProjectExternalActionTraceInput>,
    },
    Event {
        action: ProjectExternalEffectRefInput,
        source: ProjectExternalActionTraceEventSourceInput,
    },
    ParameterCall {
        parameter: etas_package_metadata::CallableRequestTarget,
    },
    Seq(Vec<ProjectExternalActionTraceInput>),
    Choice(Vec<ProjectExternalActionTraceInput>),
    Repeat(Box<ProjectExternalActionTraceInput>),
    UnknownOrder(Vec<ProjectExternalEffectRefInput>),
    Widened {
        actions: Vec<ProjectExternalEffectRefInput>,
        parameter_calls: Vec<etas_package_metadata::CallableRequestTarget>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalActionTraceEventSourceInput {
    Perform,
    StdIntrinsic,
    AgentCall,
    ExternalMetadata,
    Transfer,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalLatentFlowSummaryInput {
    pub residual_checks: Vec<etas_code::residual::identity::ResidualObligationRef>,
    pub declared_bound: ProjectExternalEffectRowInput,
    pub inferred_effects: ProjectExternalEffectRowInput,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalActionSummaryInput {
    pub action: Vec<String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalTraceSpecSummaryInput {
    pub trace_spec: Vec<String>,
    pub definition: etas_code::DefinitionKey,
    pub alternatives: Vec<Vec<ProjectExternalTraceSpecClauseInput>>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalTraceSpecClauseInput {
    pub kind: ProjectExternalTraceSpecClauseKindInput,
    pub pattern: Option<ProjectExternalEffectRowInput>,
    pub guard: Option<ProjectExternalEffectRowInput>,
    pub target: Option<ProjectExternalEffectRowInput>,
    pub obligation: Option<ProjectExternalEffectRowInput>,
    pub selector_bounds: Vec<ProjectExternalTraceSelectorBoundInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalTraceSelectorBoundInput {
    pub operand: ProjectExternalTraceSelectorOperandInput,
    pub effect_path: Vec<String>,
    pub argument: u32,
    pub binding: String,
    pub spec_path: Vec<String>,
    pub spec_args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalTraceSelectorOperandInput {
    Pattern,
    Guard,
    Target,
    Obligation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub enum ProjectExternalTraceSpecClauseKindInput {
    Allow,
    Deny,
    RequireBefore,
    RequireAfter,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalReExportInput {
    pub from: Vec<String>,
    pub exported: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectExternalExportInput {
    pub symbol: ExternalSymbolId,
    pub name: String,
    pub visibility: etas_hir::Visibility,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectToolBindingInput {
    pub tool: String,
    pub provider: String,
    pub effect_row: Vec<String>,
    pub action_row: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectEntry {
    pub module: Option<ModulePath>,
    pub flow: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, etas_core::RetainedSize)]
pub struct ModulePath {
    pub segments: Vec<String>,
}

impl ModulePath {
    pub fn from_ast(path: &ast::Path) -> Self {
        Self {
            segments: path
                .segments
                .iter()
                .map(|segment| segment.text.clone())
                .collect(),
        }
    }

    pub fn synthetic_single_file() -> Self {
        Self {
            segments: vec!["main".to_owned()],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticDumpOptions {
    pub include_labels: bool,
    pub include_notes: bool,
    pub include_suggestions: bool,
}

impl Default for DiagnosticDumpOptions {
    fn default() -> Self {
        Self {
            include_labels: true,
            include_notes: true,
            include_suggestions: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticDocument {
    pub diagnostics: Vec<DiagnosticRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticRecord {
    pub code: String,
    pub phase: String,
    pub severity: String,
    pub message: String,
    pub primary: DiagnosticSpanRecord,
    pub labels: Vec<DiagnosticLabelRecord>,
    pub notes: Vec<String>,
    pub help: Option<String>,
    pub suggestions: Vec<DiagnosticSuggestionRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticSpanRecord {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticLabelRecord {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
    pub style: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticSuggestionRecord {
    pub title: String,
    pub applicability: String,
    pub edits: Vec<DiagnosticTextEditRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct DiagnosticTextEditRecord {
    pub start: u32,
    pub end: u32,
    pub replacement: String,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct CheckOutput {
    pub checked: Option<CheckedProgram>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Option<SourceBundle>,
    pub parsed_sources: Vec<ParsedSource>,
    pub parse: Option<ParseOutput>,
    pub hir: Option<HirOutput>,
    pub types: Option<TypeOutput>,
    pub effects: Option<SharedEffectOutput>,
    pub concurrency: Option<Arc<crate::ConcurrencyFacts>>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ProjectOutput {
    pub checked: Option<CheckedProject>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Option<std::sync::Arc<SourceSet>>,
    pub parsed_sources: Vec<ParsedSource>,
    pub reused_parsed_sources: usize,
    pub modules: Option<std::sync::Arc<ModuleIndex>>,
    pub units: Option<std::sync::Arc<UnitTree>>,
    pub import_graph: Option<std::sync::Arc<ImportGraph>>,
    pub module_topo_order: Option<ModuleTopoOrder>,
    pub affected_modules: Option<AffectedModuleSet>,
    pub resolved_imports: Option<std::sync::Arc<ResolvedImports>>,
    pub resolved_paths: Option<std::sync::Arc<ResolvedPaths>>,
    pub hir_item_bindings: Option<HirItemBindings>,
    pub hir_body_bindings: Option<HirBodyBindings>,
    pub hir: Option<HirOutput>,
    pub signature_types: Option<TypeOutput>,
    pub top_level_lets: Option<TopLevelLetFacts>,
    pub type_body_outputs: HashMap<UnitId, Arc<etas_types::BodyTypeOutput>>,
    pub types: Option<TypeOutput>,
    pub effect_body_outputs: HashMap<UnitId, Arc<etas_effects::BodyEffectOutput>>,
    pub effect_pipeline_artifacts: Option<Arc<EffectPipelineArtifacts>>,
    pub effects: Option<SharedEffectOutput>,
    pub concurrency: Option<Arc<crate::ConcurrencyFacts>>,
    pub entry: Option<ProjectEntryFact>,
    pub reachability: Option<crate::ReachabilityFacts>,
    pub runtime_source_requirements: Option<crate::RuntimeSourceRequirements>,
}

impl ProjectOutput {
    pub fn into_check_output(self) -> CheckOutput {
        let sources = self
            .sources
            .as_ref()
            .map(|sources| sources.to_source_bundle());
        let parse = self
            .parsed_sources
            .first()
            .cloned()
            .map(|parsed| ParseOutput {
                source: (*parsed.source_file).clone(),
                parsed: parsed.parse,
            });

        CheckOutput {
            checked: self.checked,
            diagnostics: self.diagnostics,
            sources,
            parsed_sources: self.parsed_sources,
            parse,
            hir: self.hir,
            types: self.types,
            effects: self.effects,
            concurrency: self.concurrency,
        }
    }
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct CheckedProgramData {
    pub(crate) reflection_targets: Arc<crate::code::reflection::CheckedReflectionTargets>,
    pub(crate) foreign_type_bindings: Arc<crate::code::definition::ForeignTypeBindings>,
    pub(crate) native_instantiations: Arc<crate::project::instantiation::NativeInstantiationPlan>,
    pub compiler_version: String,
    pub std_registry: std::sync::Arc<etas_std::StdRegistry>,
    pub(crate) checked_std_library_implementations:
        std::sync::Arc<crate::passes::CheckedStandardLibraryImplementations>,
    pub project_environment_fingerprint: String,
    pub dependency_metadata_fingerprints: Vec<(String, String)>,
    pub sources: SourceBundle,
    pub module_index: Arc<ModuleIndex>,
    pub hir_item_bindings: Arc<HirItemBindings>,
    pub resolved_imports: Arc<ResolvedImports>,
    pub definition_sources: Arc<crate::code::definition::DefinitionSources>,
    pub(crate) definition_environment: Arc<ValidatedExternalEnvironment>,
    // Native candidate source declarations execute through their frozen program,
    // not through this revision's local body/intrinsic reachability closure.
    pub(crate) frozen_agent_call_boundaries: std::collections::BTreeSet<HirItemId>,
    pub compile_options: ProjectCompileOptions,
    pub optimization_report: crate::HirOptimizationReport,
    pub hir: etas_hir::HirRevision,
    pub top_level_lets: TopLevelLetFacts,
    pub types: Arc<TypeFacts>,
    pub type_store: Arc<TypeStore>,
    pub effects: Arc<EffectFacts>,
    pub concurrency: Arc<crate::ConcurrencyFacts>,
    pub cpu_parallel: Arc<crate::CpuParallelEligibilityFacts>,
    pub effect_registry: Arc<EffectRegistry>,
    pub external_tool_schemas: Vec<ProjectExternalToolSchemaInput>,
    pub entry_fact: ProjectEntryFact,
    pub entry: Option<HirItemId>,
    pub reachability: crate::ReachabilityFacts,
    /// Verified, target-supported residual plans; frontend-constructed only.
    pub(crate) residual: Arc<crate::residual::CheckedResidualPlans>,
}

impl std::ops::Deref for CheckedProgramData {
    type Target = HirProgram;

    fn deref(&self) -> &Self::Target {
        &self.hir
    }
}

impl CheckedProgramData {
    pub fn requires_definition_catalog(&self) -> bool {
        if !self.types.agent_inferences.is_empty() {
            return true;
        }
        !self.types.reflections.is_empty()
            || self.types.std_intrinsic_calls.values().any(|fact| {
                matches!(
                    fact.intrinsic,
                    etas_std::intrinsic::runtime::TOOL_REFERENCE
                        | etas_std::intrinsic::runtime::AGENT_STEP
                        | etas_std::intrinsic::runtime::TOOL_INVOKE
                )
            })
    }

    /// Residual plans every execution of this program must install. Plans are
    /// descriptions of required enforcement, not installation evidence.
    pub fn residual_plans(&self) -> &crate::residual::CheckedResidualPlans {
        &self.residual
    }

    /// Checks against the validated environment captured by this program.
    pub fn uses_environment(&self, environment: &ProjectEnvironmentInput) -> bool {
        self.definition_environment.environment() == environment
    }

    pub fn checked_reflections(
        &self,
    ) -> &std::collections::HashMap<etas_hir::HirExprId, etas_types::CheckedReflectionFact> {
        self.reflection_targets.facts()
    }

    pub fn reflection_site(
        &self,
        expression: etas_hir::HirExprId,
    ) -> Option<(etas_hir::HirItemId, etas_core::Span)> {
        self.reflection_targets.site(expression)
    }

    pub fn reflection_resolution(
        &self,
        expression: etas_hir::HirExprId,
    ) -> Option<&crate::HirPathResolution> {
        self.reflection_targets.resolution(expression)
    }

    pub fn external_definition_origins(&self) -> &[ProjectExternalDefinitionOriginsInput] {
        &self
            .definition_environment
            .environment()
            .external_definition_origins
    }
}

pub type CheckedProgram = Arc<CheckedProgramData>;
pub type CheckedProject = CheckedProgram;

/// Finalized effect facts are shared by outputs, snapshots and cache entries.
/// Mutable stage output remains owned by the effect pipeline until publication.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SharedEffectOutput {
    pub facts: Arc<EffectFacts>,
    pub diagnostics: Vec<Diagnostic>,
}

impl From<etas_effects::EffectOutput> for SharedEffectOutput {
    fn from(output: etas_effects::EffectOutput) -> Self {
        Self {
            facts: Arc::new(output.facts),
            diagnostics: output.diagnostics,
        }
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct TopLevelLetFacts {
    pub items: std::collections::HashMap<HirItemId, TopLevelLetFact>,
    pub symbols: std::collections::HashMap<SymbolId, TopLevelLetFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct TopLevelLetFact {
    pub ty: TypeId,
    pub classification: TopLevelLetClassification,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct ProjectEntryFact {
    pub requested: ProjectEntry,
    pub resolved: Option<ProjectEntryResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct ProjectEntryResolution {
    pub module: HirModuleId,
    pub item: HirItemId,
}

#[cfg(test)]
mod optimization_level_tests {
    use super::{OptimizationLevel, ProjectCompileOptions};

    #[test]
    fn optimization_level_contract_is_ordered_parsed_and_defaults_to_o2() {
        assert_eq!(OptimizationLevel::default(), OptimizationLevel::O2);
        assert!(OptimizationLevel::O3.enables(OptimizationLevel::O2));
        assert!(!OptimizationLevel::O1.enables(OptimizationLevel::O2));
        for (number, level) in [
            (0, OptimizationLevel::O0),
            (1, OptimizationLevel::O1),
            (2, OptimizationLevel::O2),
            (3, OptimizationLevel::O3),
        ] {
            assert_eq!(OptimizationLevel::try_from(number).unwrap(), level);
            assert_eq!(
                number.to_string().parse::<OptimizationLevel>().unwrap(),
                level
            );
            assert_eq!(level.to_string(), format!("O{number}"));
        }
        assert!(OptimizationLevel::try_from(4).is_err());
        assert!("-O2".parse::<OptimizationLevel>().is_err());
    }

    #[test]
    fn fingerprints_isolate_every_level_and_policy_input() {
        let fingerprints = [
            OptimizationLevel::O0,
            OptimizationLevel::O1,
            OptimizationLevel::O2,
            OptimizationLevel::O3,
        ]
        .map(|optimization_level| {
            ProjectCompileOptions {
                optimization_level,
                ..ProjectCompileOptions::default()
            }
            .canonical_options_fingerprint()
        });
        let distinct = fingerprints
            .iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(distinct.len(), fingerprints.len());

        let base = ProjectCompileOptions::default().canonical_options_fingerprint();
        let policy = ProjectCompileOptions {
            quality_context_token_budget: Some(8),
            ..ProjectCompileOptions::default()
        }
        .canonical_options_fingerprint();
        assert_ne!(base, policy);
    }
}
