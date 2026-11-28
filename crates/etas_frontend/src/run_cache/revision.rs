//! Runtime-owned HIR, binding facts and source locations for one linked revision.
use crate::{
    CheckedProgramData, StandardLibraryInterface,
    code::{
        definition::{BodyExposure, DefinitionSources},
        identity::{DefinitionOrigins, SourceModuleOrigin},
    },
};
use etas_utils::ProfileHandle;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::Arc,
};

#[derive(Serialize, Deserialize)]
pub(super) struct Revision {
    hir: Arc<etas_hir::HirProgram>,
    types: Arc<etas_types::TypeFacts>,
    type_store: Arc<etas_types::TypeStore>,
    effects: Arc<etas_effects::EffectFacts>,
    effect_registry: Arc<etas_effects::EffectRegistry>,
    instances: Arc<crate::project::instantiation::NativeInstantiationPlan>,
    concurrency: Arc<crate::ConcurrencyFacts>,
    cpu_parallel: Arc<crate::CpuParallelEligibilityFacts>,
    implementations: Arc<crate::passes::CheckedStandardLibraryImplementations>,
    modules: Arc<crate::ModuleIndex>,
    items: Arc<crate::HirItemBindings>,
    imports: Arc<crate::ResolvedImports>,
    reflection: crate::ResolvedPaths,
    top_level_lets: crate::TopLevelLetFacts,
    entry: crate::ProjectEntryFact,
    reachability: crate::ReachabilityFacts,
    options: crate::ProjectCompileOptions,
    optimization: crate::HirOptimizationReport,
    env: Environment,
    stable_source_keys: Vec<crate::ProjectStableSourceKeyInput>,
    environment_fingerprint: String,
    external_modules_fingerprint: String,
    dependency_fingerprints: Vec<(String, String)>,
    project_root: PathBuf,
    source_root: PathBuf,
    pub(super) sources: Vec<crate::SourceInput>,
    bodies: std::collections::BTreeMap<etas_hir::HirItemId, crate::AstBodyRef>,
    exposure: HashMap<etas_hir::HirItemId, BodyExposure>,
    origins: HashMap<etas_hir::HirModuleId, SourceModuleOrigin>,
}

impl Revision {
    pub(super) fn capture(p: &CheckedProgramData) -> Option<Self> {
        if !p.residual.is_empty() || !p.frozen_agent_call_boundaries.is_empty() {
            return None;
        }
        let sources = p.definition_sources.source_set();
        let mut hir = p.hir.program().clone();
        // Execution and diagnostics use forward origins. The inverse syntax
        // indexes belong to frontend/editor queries.
        hir.source_map.syntax_nodes = Default::default();
        hir.source_map.syntax_to_hir = Default::default();
        Some(Self {
            hir: Arc::new(hir),
            types: p.types.clone(),
            type_store: p.type_store.clone(),
            effects: p.effects.clone(),
            effect_registry: p.effect_registry.clone(),
            instances: p.native_instantiations.clone(),
            concurrency: p.concurrency.clone(),
            cpu_parallel: p.cpu_parallel.clone(),
            implementations: p.checked_std_library_implementations.clone(),
            modules: p.module_index.clone(),
            items: p.hir_item_bindings.clone(),
            imports: p.resolved_imports.clone(),
            reflection: p.reflection_targets.library_paths(&p.hir).ok()?,
            top_level_lets: p.top_level_lets.clone(),
            entry: p.entry_fact.clone(),
            reachability: p.reachability.clone(),
            options: p.compile_options.clone(),
            optimization: p.optimization_report.clone(),
            env: Environment::capture(p)?,
            environment_fingerprint: p.project_environment_fingerprint.clone(),
            external_modules_fingerprint: sources.external_modules_fingerprint.clone(),
            stable_source_keys: sources.stable_source_keys.clone(),
            dependency_fingerprints: p.dependency_metadata_fingerprints.clone(),
            project_root: sources.project_root.clone(),
            source_root: sources.source_root.clone(),
            sources: sources
                .files
                .iter()
                .map(|s| crate::SourceInput {
                    id: s.id,
                    path: s.path.clone(),
                    text: s.text.to_string(),
                    kind: s.kind.clone(),
                })
                .collect(),
            bodies: p.definition_sources.library_body_slots().ok()?,
            exposure: p.definition_sources.installed_exposure(),
            origins: DefinitionOrigins::checked_source_modules(p).ok()?,
        })
    }

    pub(super) fn restore(
        self,
        registry: Arc<etas_std::StdRegistry>,
        profile: &ProfileHandle,
        interface: Option<&StandardLibraryInterface>,
    ) -> Option<CheckedProgramData> {
        let env = Arc::new(crate::ValidatedExternalEnvironment::from_checked(
            self.env.restore(interface)?,
            self.environment_fingerprint,
        ));
        let sources = Arc::new(crate::SourceSet {
            project_root: self.project_root,
            source_root: self.source_root,
            environment_fingerprint: env.fingerprint().to_owned(),
            external_modules_fingerprint: self.external_modules_fingerprint,
            stable_source_keys: self.stable_source_keys,
            files: self
                .sources
                .iter()
                .map(crate::SourceFile::from_input)
                .collect(),
        });
        let archive = DefinitionSources::installed(
            sources.clone(),
            self.bodies.into_iter().collect(),
            self.exposure,
            self.origins,
            profile.clone(),
        );
        let hir = etas_hir::HirRevision::from_shared_program(self.hir).ok()?;
        let targets = crate::code::reflection::CheckedReflectionTargets::resolve(
            &hir,
            &self.reflection,
            &etas_types::TypeOutput {
                store: self.type_store.clone(),
                facts: self.types.clone(),
                diagnostics: Vec::new(),
            },
            &self.reachability.analysis_items,
            false,
            None,
        )
        .ok()?;
        Some(CheckedProgramData {
            entry_run_cache: None,
            definition_catalog_cache: Default::default(),
            semantic_dependencies: Default::default(),
            reflection_targets: Arc::new(targets),
            foreign_type_bindings: Default::default(),
            native_instantiations: self.instances,
            compiler_version: crate::FRONTEND_COMPILER_VERSION.to_owned(),
            std_registry: registry,
            standard_library: None,
            standard_library_interface: None,
            toolchain_base: None,
            checked_std_library_implementations: self.implementations,
            project_environment_fingerprint: env.fingerprint().to_owned(),
            dependency_metadata_fingerprints: self.dependency_fingerprints,
            sources: sources.to_source_bundle(),
            module_index: self.modules,
            hir_item_bindings: self.items,
            resolved_imports: self.imports,
            definition_sources: Arc::new(archive),
            external_tool_schemas: env
                .environment()
                .external_public_metadata
                .iter()
                .flat_map(|m| m.tool_schemas.iter().cloned())
                .collect(),
            definition_environment: env,
            frozen_agent_call_boundaries: BTreeSet::new(),
            residual: Arc::new(crate::residual::CheckedResidualPlans::empty_for_target(
                self.options.residual_target.clone(),
            )),
            compile_options: self.options,
            optimization_report: self.optimization,
            hir,
            top_level_lets: self.top_level_lets,
            types: self.types,
            type_store: self.type_store,
            effects: self.effects,
            concurrency: self.concurrency,
            cpu_parallel: self.cpu_parallel,
            effect_registry: self.effect_registry,
            entry: self.entry.resolved.as_ref().map(|e| e.item),
            entry_fact: self.entry,
            reachability: self.reachability,
        })
    }
}

// Tagged package metadata has a self-describing wire representation. The
// immutable SDK descriptors are already in the installed interface used by run.
#[derive(Serialize, Deserialize)]
struct Environment {
    data: Vec<u8>,
    source_bindings: Vec<etas_effects::DependencySourceEffectBinding>,
    sdk_origin: Option<(usize, crate::ExternalPackageId)>,
}

impl Environment {
    fn capture(program: &CheckedProgramData) -> Option<Self> {
        let mut env = program.definition_environment.environment().clone();
        let sdk_origin = program
            .standard_library_interface
            .as_ref()
            .and_then(|library| {
                let position = env.external_definition_origins.iter().position(|origin| {
                    origin
                        .descriptors
                        .as_ref()
                        .is_some_and(|d| Arc::ptr_eq(d, &library.descriptors))
                        && origin.origins == library.origins
                })?;
                Some((
                    position,
                    env.external_definition_origins.remove(position).package,
                ))
            });
        let mut data = Vec::new();
        ciborium::ser::into_writer(&env, &mut data).ok()?;
        Some(Self {
            data,
            sdk_origin,
            source_bindings: env.external_effect_metadata.source_bindings,
        })
    }

    fn restore(
        self,
        interface: Option<&StandardLibraryInterface>,
    ) -> Option<crate::ProjectEnvironmentInput> {
        let mut env: crate::ProjectEnvironmentInput =
            ciborium::de::from_reader(self.data.as_slice()).ok()?;
        env.external_effect_metadata.source_bindings = self.source_bindings;
        if let Some((position, package)) = self.sdk_origin {
            let interface = interface?;
            env.external_definition_origins.insert(
                position,
                crate::ProjectExternalDefinitionOriginsInput {
                    package,
                    origins: interface.origins.clone(),
                    descriptors: Some(interface.descriptors.clone()),
                },
            );
        }
        Some(env)
    }
}
