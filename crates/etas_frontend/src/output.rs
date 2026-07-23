use std::{collections::HashMap, path::PathBuf};

use etas_core::{Diagnostic, SourceId, id_type};
use etas_effects::{
    DependencyEffectMetadata, EffectFacts, EffectOutput, EffectPipelineArtifacts, EffectRegistry,
    InterpreterSupportFacts,
};
use etas_hir::{
    HirItemId, HirModuleId, HirProgram, ScopeTree, SymbolId, SymbolTable, TopLevelLetClassification,
};
use etas_syntax::ast;
use etas_types::{TypeFacts, TypeId, TypeOutput, TypeStore};
use etas_utils::UnitKindKey;

use crate::project::{
    AffectedModuleSet, HirBodyBindings, HirItemBindings, HirOutput, ImportGraph, ModuleIndex,
    ModuleTopoOrder, ParseOutput, ParsedSource, ResolvedImports, ResolvedPaths, SourceBundle,
    SourceInput, SourceSet, UnitTree,
};

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

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectCompileOptions {
    pub entry_policy: EntryPolicy,
}

impl ProjectCompileOptions {
    pub fn canonical_options_fingerprint(&self) -> String {
        let entry_policy = match self.entry_policy {
            EntryPolicy::CompileOnly => "compile-only",
            EntryPolicy::Optional => "optional",
            EntryPolicy::Runnable => "runnable",
        };
        crate::artifact::fingerprint_text(&["frontend-options", "v2", entry_policy]).to_string()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EntryPolicy {
    #[default]
    CompileOnly,
    Optional,
    Runnable,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectEnvironmentInput {
    pub external_packages: Vec<ProjectExternalPackageInput>,
    pub external_modules: Vec<ProjectExternalModuleInput>,
    pub external_public_metadata: Vec<ProjectExternalPublicMetadataInput>,
    pub external_effect_metadata: DependencyEffectMetadata,
    pub tool_bindings: Vec<ProjectToolBindingInput>,
    pub environment_fingerprint: Option<String>,
    pub external_modules_fingerprint: Option<String>,
}

impl ProjectEnvironmentInput {
    pub fn canonical_environment_fingerprint(&self) -> String {
        let mut parts = vec!["frontend_project_environment:v1".to_owned()];
        if let Some(fingerprint) = &self.environment_fingerprint {
            parts.push(format!("external-fingerprint:{fingerprint}"));
        }
        parts.push(format!(
            "external-packages:{}",
            self.canonical_external_packages_fingerprint()
        ));
        parts.push(format!(
            "external-modules:{}",
            self.canonical_external_modules_fingerprint()
        ));
        parts.push("public-metadata".to_owned());
        let mut public_metadata = self.external_public_metadata.clone();
        public_metadata.sort_by(|left, right| left.package.cmp(&right.package));
        for metadata in public_metadata {
            parts.push(format!("public-package:{}", metadata.package.0));
            for signature in metadata.types {
                parts.push(format!(
                    "type:{}:{}",
                    signature.path.join("."),
                    signature.visibility
                ));
            }
            for signature in metadata.values {
                parts.push(format!("value:{}:{signature:?}", signature.path.join(".")));
            }
            for signature in metadata.enums {
                parts.push(format!(
                    "enum:{}:{}",
                    signature.path.join("."),
                    signature.visibility
                ));
            }
            for signature in metadata.effects {
                parts.push(format!(
                    "effect:{}:{}",
                    signature.path.join("."),
                    signature.visibility
                ));
            }
            for signature in metadata.flows {
                parts.push(format!("flow:{}:{signature:?}", signature.path.join(".")));
            }
            for signature in metadata.agents {
                parts.push(format!("agent:{}:{signature:?}", signature.path.join(".")));
            }
            for signature in metadata.tools {
                parts.push(format!("tool:{}:{signature:?}", signature.path.join(".")));
            }
            for schema in metadata.tool_schemas {
                parts.push(format!(
                    "tool-schema:{}:{}",
                    schema.path.join("."),
                    schema.schema_json
                ));
            }
            for signature in metadata.actions {
                parts.push(format!("action:{}:{signature:?}", signature.path.join(".")));
            }
            for signature in metadata.spec_signatures {
                parts.push(format!(
                    "spec:{}:{:?}:{}",
                    signature.path.join("."),
                    signature.kind,
                    signature.param_names.join(",")
                ));
                for method in signature.methods {
                    parts.push(format!(
                        "spec-method:{}:{}:{:?}",
                        signature.path.join("."),
                        method.name,
                        method.signature
                    ));
                }
                for bound in signature.super_specs {
                    parts.push(format!(
                        "spec-super:{}:{}:{}",
                        signature.path.join("."),
                        bound.spec.join("."),
                        bound
                            .args
                            .iter()
                            .map(external_type_fingerprint)
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
            for implementation in metadata.spec_impls {
                parts.push(format!(
                    "spec-impl:{}:{}:{}",
                    external_type_fingerprint(&implementation.self_type),
                    implementation.spec.join("."),
                    implementation
                        .args
                        .iter()
                        .map(external_type_fingerprint)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            for fact in metadata.type_spec_satisfactions {
                parts.push(format!(
                    "type-spec-satisfaction:{}:{}:{}",
                    external_type_fingerprint(&fact.self_type),
                    fact.spec.join("."),
                    fact.args
                        .iter()
                        .map(external_type_fingerprint)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            for fact in metadata.callable_spec_satisfactions {
                parts.push(format!(
                    "callable-spec-satisfaction:{}:{}:{}",
                    fact.item.join("."),
                    fact.spec.join("."),
                    fact.args
                        .iter()
                        .map(external_type_fingerprint)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            for fact in metadata.trace_spec_conformances {
                parts.push(format!(
                    "trace-spec-conformance:{}:{}",
                    fact.item.join("."),
                    external_trace_spec_conformance_target_fingerprint(&fact.target)
                ));
            }
            for summary in metadata.effect_summaries {
                parts.push(format!(
                    "effect-summary:{}:{}:{}:{}",
                    summary.item.join("."),
                    external_effect_row_fingerprint(&summary.public_effects),
                    external_effect_row_fingerprint(&summary.requested_actions),
                    external_effect_row_fingerprint(&summary.handled_requested_actions)
                ));
                for latent in summary.latent_flows {
                    parts.push(format!(
                        "latent-flow:{}:{}:{}",
                        summary.item.join("."),
                        external_effect_row_fingerprint(&latent.declared_bound),
                        external_effect_row_fingerprint(&latent.inferred_effects)
                    ));
                }
            }
            for summary in metadata.action_summaries {
                parts.push(format!(
                    "action-summary:{}:{}",
                    summary.action.join("."),
                    summary.args.join(",")
                ));
            }
            for summary in metadata.trace_spec_summaries {
                parts.push(format!(
                    "trace-spec-summary:{}",
                    summary.trace_spec.join(".")
                ));
                for clause in summary.clauses {
                    parts.push(format!(
                        "trace-spec-clause:{}:{}:{}:{}:{}:{}",
                        summary.trace_spec.join("."),
                        clause.kind.as_str(),
                        clause
                            .pattern
                            .as_ref()
                            .map(external_effect_row_fingerprint)
                            .unwrap_or_default(),
                        clause
                            .guard
                            .as_ref()
                            .map(external_effect_row_fingerprint)
                            .unwrap_or_default(),
                        clause
                            .target
                            .as_ref()
                            .map(external_effect_row_fingerprint)
                            .unwrap_or_default(),
                        clause
                            .obligation
                            .as_ref()
                            .map(external_effect_row_fingerprint)
                            .unwrap_or_default()
                    ));
                }
            }
            for re_export in metadata.re_exports {
                parts.push(format!(
                    "re-export:{}:{}",
                    re_export.from.join("."),
                    re_export.exported.join(".")
                ));
            }
        }
        parts.push("effect-metadata".to_owned());
        for tag in &self.external_effect_metadata.tags {
            parts.push(format!(
                "tag:{}:{}",
                tag.path.join("."),
                tag.runtime_requirement
                    .as_ref()
                    .map(|requirement| format!("{requirement:?}"))
                    .unwrap_or_default()
            ));
        }
        for action in &self.external_effect_metadata.actions {
            parts.push(format!(
                "action:{}:{}:{}:{}",
                action.path.join("."),
                action
                    .effect_args
                    .iter()
                    .map(|arg| format!("{arg:?}"))
                    .collect::<Vec<_>>()
                    .join(","),
                action.returns_never,
                action
                    .runtime_requirement
                    .as_ref()
                    .map(|requirement| format!("{requirement:?}"))
                    .unwrap_or_default()
            ));
        }
        for extension in &self.external_effect_metadata.extensions {
            parts.push(format!(
                "extends:{:?}:{}:{}",
                extension.package,
                extension.child.join("."),
                extension.parent.join(".")
            ));
        }
        parts.push("tool-bindings".to_owned());
        for binding in &self.tool_bindings {
            parts.push(format!(
                "tool:{}:{}:{}:{}",
                binding.tool,
                binding.provider,
                binding.effect_row.join(","),
                binding.action_row.join(",")
            ));
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        crate::artifact::fingerprint_text(&refs).to_string()
    }

    pub fn canonical_dependency_metadata_fingerprints(&self) -> Vec<(String, String)> {
        let mut packages = self.external_packages.clone();
        packages.sort_by(|left, right| left.import_root.cmp(&right.import_root));
        packages
            .into_iter()
            .map(|package| {
                let mut parts = vec![
                    "frontend_dependency_metadata:v1".to_owned(),
                    format!("name:{}", package.name),
                    format!("version:{}", package.version),
                    format!("edition:{}", package.edition),
                    format!("import-root:{}", package.import_root),
                ];
                let mut modules = self
                    .external_modules
                    .iter()
                    .filter(|module| module.package == Some(package.id))
                    .map(|module| format!("{module:?}"))
                    .collect::<Vec<_>>();
                modules.sort();
                parts.extend(modules);
                let mut metadata = self
                    .external_public_metadata
                    .iter()
                    .filter(|metadata| metadata.package == package.id)
                    .map(|metadata| format!("{metadata:?}"))
                    .collect::<Vec<_>>();
                metadata.sort();
                parts.extend(metadata);
                parts.push(format!(
                    "dependency-effects:{:?}",
                    self.external_effect_metadata
                ));
                let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
                (
                    package.import_root,
                    crate::artifact::fingerprint_text(&refs).to_string(),
                )
            })
            .collect()
    }

    pub fn canonical_external_packages_fingerprint(&self) -> String {
        let mut parts = vec!["frontend_external_packages:v1".to_owned()];
        let mut packages = self.external_packages.clone();
        packages.sort_by(|left, right| {
            left.import_root
                .cmp(&right.import_root)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.version.cmp(&right.version))
                .then_with(|| left.id.cmp(&right.id))
        });
        for package in packages {
            parts.push(format!(
                "package:{}:{}:{}:{}:{}",
                package.id.0, package.name, package.version, package.edition, package.import_root
            ));
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        crate::artifact::fingerprint_text(&refs).to_string()
    }

    pub fn canonical_external_modules_fingerprint(&self) -> String {
        let mut parts = vec!["frontend_external_modules:v1".to_owned()];
        if let Some(fingerprint) = &self.external_modules_fingerprint {
            parts.push(format!("external-fingerprint:{fingerprint}"));
        }
        let mut modules = self.external_modules.clone();
        modules.sort_by(|left, right| {
            left.path
                .segments
                .cmp(&right.path.segments)
                .then_with(|| left.id.cmp(&right.id))
        });
        for module in modules {
            parts.push(format!(
                "module:{}:{}:{}",
                module.id.0,
                module
                    .package
                    .map(|package| package.0.to_string())
                    .unwrap_or_default(),
                module.path.segments.join(".")
            ));
            let mut exports = module.exports;
            exports.sort_by(|left, right| {
                left.name
                    .cmp(&right.name)
                    .then_with(|| left.symbol.cmp(&right.symbol))
            });
            for export in exports {
                parts.push(format!(
                    "export:{}:{}:{:?}",
                    export.symbol.0, export.name, export.visibility
                ));
            }
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        crate::artifact::fingerprint_text(&refs).to_string()
    }
}

fn external_trace_spec_conformance_target_fingerprint(
    target: &ProjectExternalTraceSpecConformanceTargetInput,
) -> String {
    match target {
        ProjectExternalTraceSpecConformanceTargetInput::Inline => "inline".to_owned(),
        ProjectExternalTraceSpecConformanceTargetInput::Named { spec, args } => format!(
            "named:{}<{}>",
            spec.join("."),
            args.iter()
                .map(external_type_fingerprint)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn external_effect_row_fingerprint(row: &ProjectExternalEffectRowInput) -> String {
    row.effects
        .iter()
        .map(|effect| {
            let args = effect
                .args
                .iter()
                .map(external_effect_arg_fingerprint)
                .collect::<Vec<_>>()
                .join(",");
            format!("{}<{args}>", effect.path.join("."))
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn external_effect_arg_fingerprint(arg: &ProjectExternalEffectArgInput) -> String {
    match arg {
        ProjectExternalEffectArgInput::Type(ty) => {
            format!("type:{}", external_type_fingerprint(ty))
        }
        ProjectExternalEffectArgInput::Path(path) => format!("path:{}", path.join(".")),
        ProjectExternalEffectArgInput::String(value) => format!("string:{value:?}"),
        ProjectExternalEffectArgInput::Wildcard => "wildcard:_".to_owned(),
    }
}

fn external_type_fingerprint(ty: &ProjectExternalTypeInput) -> String {
    match ty {
        ProjectExternalTypeInput::Primitive(name) => format!("primitive:{name}"),
        ProjectExternalTypeInput::Var(name) => format!("var:{name}"),
        ProjectExternalTypeInput::Named(path) => format!("named:{}", path.join(".")),
        ProjectExternalTypeInput::Applied { path, args } => format!(
            "applied:{}<{}>",
            path.join("."),
            args.iter()
                .map(external_type_fingerprint)
                .collect::<Vec<_>>()
                .join(",")
        ),
        ProjectExternalTypeInput::Alias { path, target } => {
            format!(
                "alias:{}={}",
                path.join("."),
                external_type_fingerprint(target)
            )
        }
        ProjectExternalTypeInput::Nominal {
            path,
            representation,
        } => format!(
            "nominal:{}={}",
            path.join("."),
            representation
                .as_ref()
                .map(|ty| external_type_fingerprint(ty))
                .unwrap_or_default()
        ),
        ProjectExternalTypeInput::Array(element) => {
            format!("array:{}", external_type_fingerprint(element))
        }
        ProjectExternalTypeInput::List(element) => {
            format!("list:{}", external_type_fingerprint(element))
        }
        ProjectExternalTypeInput::Map { key, value } => format!(
            "map:{}=>{}",
            external_type_fingerprint(key),
            external_type_fingerprint(value)
        ),
        ProjectExternalTypeInput::Set(element) => {
            format!("set:{}", external_type_fingerprint(element))
        }
        ProjectExternalTypeInput::Range(index) => {
            format!("range:{}", external_type_fingerprint(index))
        }
        ProjectExternalTypeInput::Slice(element) => {
            format!("slice:{}", external_type_fingerprint(element))
        }
        ProjectExternalTypeInput::Option(inner) => {
            format!("option:{}", external_type_fingerprint(inner))
        }
        ProjectExternalTypeInput::Result { ok, err } => format!(
            "result:{}|{}",
            external_type_fingerprint(ok),
            external_type_fingerprint(err)
        ),
        ProjectExternalTypeInput::Record { fields } => format!(
            "record:{{{}}}",
            fields
                .iter()
                .map(|field| format!("{}:{}", field.name, external_type_fingerprint(&field.ty)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        ProjectExternalTypeInput::Tuple(elements) => format!(
            "tuple:({})",
            elements
                .iter()
                .map(external_type_fingerprint)
                .collect::<Vec<_>>()
                .join(",")
        ),
        ProjectExternalTypeInput::Function {
            input,
            output,
            effects,
        } => format!(
            "function:({})->{}!{}",
            input
                .iter()
                .map(external_type_fingerprint)
                .collect::<Vec<_>>()
                .join(","),
            external_type_fingerprint(output),
            effects
                .as_ref()
                .map(external_effect_row_fingerprint)
                .unwrap_or_default()
        ),
        ProjectExternalTypeInput::Handler {
            handled,
            produced,
            result,
        } => format!(
            "handler:{}=>{} for {}",
            external_effect_row_fingerprint(handled),
            produced
                .as_ref()
                .map(external_effect_row_fingerprint)
                .unwrap_or_default(),
            result
                .as_ref()
                .map(|ty| external_type_fingerprint(ty))
                .unwrap_or_default()
        ),
        ProjectExternalTypeInput::Trust { wrapper, inner } => {
            format!("trust:{wrapper:?}:{}", external_type_fingerprint(inner))
        }
        ProjectExternalTypeInput::Prompt => "prompt".to_owned(),
        ProjectExternalTypeInput::PromptPart => "prompt_part".to_owned(),
        ProjectExternalTypeInput::Message(inner) => {
            format!("message:{}", external_type_fingerprint(inner))
        }
        ProjectExternalTypeInput::MemorySelection(inner) => {
            format!("memory_selection:{}", external_type_fingerprint(inner))
        }
        ProjectExternalTypeInput::Store { key, value } => format!(
            "store:{}=>{}",
            external_type_fingerprint(key),
            external_type_fingerprint(value)
        ),
        ProjectExternalTypeInput::MemoryRegion(schema) => {
            format!("memory_region:{}", external_type_fingerprint(schema))
        }
        ProjectExternalTypeInput::ResourceHandle { name, args } => format!(
            "resource:{name}<{}>",
            args.iter()
                .map(external_type_fingerprint)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

#[derive(Clone, Debug)]
pub struct ProjectExternalPackageInput {
    pub id: ExternalPackageId,
    pub name: String,
    pub version: String,
    pub edition: String,
    pub import_root: String,
}

#[derive(Clone, Debug)]
pub struct ProjectExternalModuleInput {
    pub package: Option<ExternalPackageId>,
    pub id: ExternalModuleId,
    pub path: ModulePath,
    pub exports: Vec<ProjectExternalExportInput>,
}

#[derive(Clone, Debug)]
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
    pub type_spec_satisfactions: Vec<ProjectExternalTypeSpecSatisfactionInput>,
    pub callable_spec_satisfactions: Vec<ProjectExternalCallableSpecSatisfactionInput>,
    pub trace_spec_conformances: Vec<ProjectExternalTraceSpecConformanceInput>,
    pub effect_summaries: Vec<ProjectExternalEffectSummaryInput>,
    pub action_summaries: Vec<ProjectExternalActionSummaryInput>,
    pub trace_spec_summaries: Vec<ProjectExternalTraceSpecSummaryInput>,
    pub re_exports: Vec<ProjectExternalReExportInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalNamedSignatureInput {
    pub path: Vec<String>,
    pub visibility: String,
    pub ty: Option<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalSpecSignatureInput {
    pub path: Vec<String>,
    pub visibility: String,
    pub kind: ProjectExternalSpecKindInput,
    pub param_names: Vec<String>,
    pub callable: Option<ProjectExternalFlowSignatureInput>,
    pub methods: Vec<ProjectExternalSpecMethodInput>,
    pub super_specs: Vec<ProjectExternalSpecBoundInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectExternalSpecKindInput {
    Type,
    Callable,
    Trace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalSpecMethodInput {
    pub name: String,
    pub path: Vec<String>,
    pub signature: Option<ProjectExternalFlowSignatureInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalSpecBoundInput {
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalSpecImplInput {
    pub self_type: ProjectExternalTypeInput,
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
    pub methods: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalTypeSpecSatisfactionInput {
    pub self_type: ProjectExternalTypeInput,
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalCallableSpecSatisfactionInput {
    pub item: Vec<String>,
    pub spec: Vec<String>,
    pub args: Vec<ProjectExternalTypeInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalTraceSpecConformanceInput {
    pub item: Vec<String>,
    pub target: ProjectExternalTraceSpecConformanceTargetInput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectExternalTraceSpecConformanceTargetInput {
    Inline,
    Named {
        spec: Vec<String>,
        args: Vec<ProjectExternalTypeInput>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalFlowSignatureInput {
    pub path: Vec<String>,
    pub param_names: Vec<String>,
    pub params: Vec<ProjectExternalTypeInput>,
    pub output: ProjectExternalTypeInput,
    pub effects: Option<ProjectExternalEffectRowInput>,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalAgentSignatureInput {
    pub path: Vec<String>,
    pub param_names: Vec<String>,
    pub input: Vec<ProjectExternalTypeInput>,
    pub output: ProjectExternalTypeInput,
    pub effects: Option<ProjectExternalEffectRowInput>,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalToolSignatureInput {
    pub path: Vec<String>,
    pub param_names: Vec<String>,
    pub input: Vec<ProjectExternalTypeInput>,
    pub output: ProjectExternalTypeInput,
    pub effects: Option<ProjectExternalEffectRowInput>,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalToolSchemaInput {
    pub path: Vec<String>,
    pub schema_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalActionSignatureInput {
    pub path: Vec<String>,
    pub params: Vec<ProjectExternalTypeInput>,
    pub effect_args: Vec<ProjectExternalActionArgKindInput>,
    pub selector_param_names: Vec<String>,
    pub selector_defaults: Vec<Option<ProjectExternalEffectArgInput>>,
    pub output: ProjectExternalTypeInput,
    pub returns_never: bool,
    pub visibility: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectExternalActionArgKindInput {
    Type,
    MemoryPlace,
    StaticResourcePath { ty: String },
    StringPattern,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectExternalEffectRowInput {
    pub effects: Vec<ProjectExternalEffectRefInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalEffectRefInput {
    pub path: Vec<String>,
    pub args: Vec<ProjectExternalEffectArgInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectExternalEffectArgInput {
    Type(ProjectExternalTypeInput),
    Path(Vec<String>),
    String(String),
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectExternalTypeInput {
    Primitive(String),
    Var(String),
    Named(Vec<String>),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalRecordFieldInput {
    pub name: String,
    pub ty: ProjectExternalTypeInput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalEffectSummaryInput {
    pub item: Vec<String>,
    pub public_effects: ProjectExternalEffectRowInput,
    pub requested_actions: ProjectExternalEffectRowInput,
    pub handled_requested_actions: ProjectExternalEffectRowInput,
    pub latent_flows: Vec<ProjectExternalLatentFlowSummaryInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalLatentFlowSummaryInput {
    pub declared_bound: ProjectExternalEffectRowInput,
    pub inferred_effects: ProjectExternalEffectRowInput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalActionSummaryInput {
    pub action: Vec<String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalTraceSpecSummaryInput {
    pub trace_spec: Vec<String>,
    pub clauses: Vec<ProjectExternalTraceSpecClauseInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalTraceSpecClauseInput {
    pub kind: ProjectExternalTraceSpecClauseKindInput,
    pub pattern: Option<ProjectExternalEffectRowInput>,
    pub guard: Option<ProjectExternalEffectRowInput>,
    pub target: Option<ProjectExternalEffectRowInput>,
    pub obligation: Option<ProjectExternalEffectRowInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectExternalTraceSpecClauseKindInput {
    Allow,
    Deny,
    RequireBefore,
    RequireAfter,
}

impl ProjectExternalTraceSpecClauseKindInput {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::RequireBefore => "require_before",
            Self::RequireAfter => "require_after",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectExternalReExportInput {
    pub from: Vec<String>,
    pub exported: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectExternalExportInput {
    pub symbol: ExternalSymbolId,
    pub name: String,
    pub visibility: etas_hir::Visibility,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectToolBindingInput {
    pub tool: String,
    pub provider: String,
    pub effect_row: Vec<String>,
    pub action_row: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectEntry {
    pub module: Option<ModulePath>,
    pub flow: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticDocument {
    pub diagnostics: Vec<DiagnosticRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticSpanRecord {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticLabelRecord {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
    pub style: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticSuggestionRecord {
    pub title: String,
    pub applicability: String,
    pub edits: Vec<DiagnosticTextEditRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticTextEditRecord {
    pub start: u32,
    pub end: u32,
    pub replacement: String,
}

#[derive(Clone, Debug)]
pub struct CheckOutput {
    pub checked: Option<CheckedProgram>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Option<SourceBundle>,
    pub parsed_sources: Vec<ParsedSource>,
    pub parse: Option<ParseOutput>,
    pub hir: Option<HirOutput>,
    pub types: Option<TypeOutput>,
    pub effects: Option<EffectOutput>,
}

#[derive(Clone, Debug)]
pub struct ProjectOutput {
    pub checked: Option<CheckedProject>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Option<SourceSet>,
    pub parsed_sources: Vec<ParsedSource>,
    pub reused_parsed_sources: usize,
    pub modules: Option<ModuleIndex>,
    pub units: Option<UnitTree>,
    pub import_graph: Option<ImportGraph>,
    pub module_topo_order: Option<ModuleTopoOrder>,
    pub affected_modules: Option<AffectedModuleSet>,
    pub resolved_imports: Option<ResolvedImports>,
    pub resolved_paths: Option<ResolvedPaths>,
    pub hir_item_bindings: Option<HirItemBindings>,
    pub hir_body_bindings: Option<HirBodyBindings>,
    pub hir: Option<HirOutput>,
    pub signature_types: Option<TypeOutput>,
    pub top_level_lets: Option<TopLevelLetFacts>,
    pub type_body_outputs: HashMap<UnitId, TypeOutput>,
    pub types: Option<TypeOutput>,
    pub effect_body_outputs: HashMap<UnitId, EffectOutput>,
    pub effect_pipeline_artifacts: Option<EffectPipelineArtifacts>,
    pub effects: Option<EffectOutput>,
    pub entry: Option<ProjectEntryFact>,
    pub reachability: Option<crate::ReachabilityFacts>,
    pub runtime_source_requirements: Option<crate::RuntimeSourceRequirements>,
}

impl ProjectOutput {
    pub fn into_check_output(self) -> CheckOutput {
        let sources = self.sources.as_ref().map(SourceSet::to_source_bundle);
        let parse = self
            .parsed_sources
            .first()
            .cloned()
            .map(|parsed| ParseOutput {
                source: parsed.source_file,
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
        }
    }
}

#[derive(Clone, Debug)]
pub struct CheckedProgram {
    pub compiler_version: String,
    pub std_registry: std::sync::Arc<etas_std::StdRegistry>,
    pub project_environment_fingerprint: String,
    pub dependency_metadata_fingerprints: Vec<(String, String)>,
    pub sources: SourceBundle,
    pub module_index: ModuleIndex,
    pub hir: HirProgram,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub source_map: etas_hir::SourceMap,
    pub top_level_lets: TopLevelLetFacts,
    pub types: TypeFacts,
    pub type_store: TypeStore,
    pub effects: EffectFacts,
    pub effect_registry: EffectRegistry,
    pub interpreter_support: InterpreterSupportFacts,
    pub external_tool_schemas: Vec<ProjectExternalToolSchemaInput>,
    pub entry_fact: ProjectEntryFact,
    pub entry: Option<HirItemId>,
    pub reachability: crate::ReachabilityFacts,
}

pub type CheckedProject = CheckedProgram;

#[derive(Clone, Debug, Default)]
pub struct TopLevelLetFacts {
    pub items: std::collections::HashMap<HirItemId, TopLevelLetFact>,
    pub symbols: std::collections::HashMap<SymbolId, TopLevelLetFact>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopLevelLetFact {
    pub ty: TypeId,
    pub classification: TopLevelLetClassification,
}

#[derive(Clone, Debug)]
pub struct ProjectEntryFact {
    pub requested: ProjectEntry,
    pub resolved: Option<ProjectEntryResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectEntryResolution {
    pub module: HirModuleId,
    pub item: HirItemId,
}
