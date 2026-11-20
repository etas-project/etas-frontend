//! Direct publication of an SDK authenticated by its installing toolchain.
//!
//! All static checks already succeeded before this payload was emitted. Loading
//! only binds source archives and reflection owners to this process. Residual
//! monitor programs use the ordinary proof-rebinding path instead of this format.

use std::{path::Path, sync::Arc};

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassControl, PassDescriptor, PassKind, PassManager, PassResult,
    Pipeline, PreservedArtifacts, UnitOrder, UnitSelector,
};
use serde::{Deserialize, Serialize};

use super::{LibraryHirImage, LibraryHirImageError, decode_prepared_value, error};
use crate::{
    CheckedProgramData, ProjectContext, ProjectInput, SourceInput,
    passes::{self, artifacts::*},
};

pub(super) const MAGIC: &[u8; 8] = b"ETASPUB1";

#[derive(Serialize, Deserialize)]
struct InstalledPublication {
    implementations: crate::passes::CheckedStandardLibraryImplementations,
}

pub(super) fn encode(checked: &CheckedProgramData) -> Result<Vec<u8>, LibraryHirImageError> {
    if checked.compile_options.standard_library_mode != crate::StandardLibraryMode::SourceBootstrap
        || checked.standard_library.is_some()
        || !checked.frozen_agent_call_boundaries.is_empty()
    {
        return Err(error(
            "installed publication requires a standalone checked standard library",
        ));
    }
    let publication = InstalledPublication {
        implementations: checked.checked_std_library_implementations.as_ref().clone(),
    };
    let mut bytes = MAGIC.to_vec();
    bytes.extend(
        bincode::serde::encode_to_vec(&publication, bincode::config::standard()).map_err(error)?,
    );
    Ok(bytes)
}

pub(super) fn load(
    mut image: LibraryHirImage,
    root: &Path,
    payload: &[u8],
    profile: &etas_utils::ProfileHandle,
) -> Result<crate::CheckedProgram, LibraryHirImageError> {
    let _load = profile.span("std.load_installed_program", "std");
    let publication: InstalledPublication = decode_prepared_value(payload)?;
    if image.schema != super::SCHEMA || image.compiler_version != crate::FRONTEND_COMPILER_VERSION {
        return Err(error("incompatible installed library image"));
    }
    let registry = Arc::new(
        image
            .native_registry
            .clone()
            .into_registry()
            .map_err(error)?,
    );
    if image.std_contract != registry.version().compatibility_identity() {
        return Err(error(
            "installed library and toolchain standard contracts differ",
        ));
    }
    let emit = PublishInstalledLibraryPass {
        semantic_dependencies: image.semantic_dependencies.clone(),
        publication,
        effects: std::mem::take(&mut image.effects),
        registry: std::mem::take(&mut image.effect_registry),
        concurrency: std::mem::take(&mut image.concurrency),
        cpu_parallel: std::mem::take(&mut image.cpu_parallel),
        instantiations: std::mem::take(&mut image.native_instantiations),
    };
    let input = ProjectInput {
        project_root: root.to_owned(),
        source_root: Some(root.to_owned()),
        options: image.compile_options.clone(),
        environment: image.environment.clone(),
        sources: image
            .sources
            .iter()
            .map(|source| SourceInput {
                id: source.id,
                path: Some(root.join(&source.relative_path)),
                text: source.text.clone(),
                kind: source.kind.clone(),
            })
            .collect(),
        entry: image.entry.requested.clone(),
    };
    let pipeline = Pipeline::new("frontend.load_installed_library")
        .pass(passes::BuildSourceSetPass)
        .for_each(
            UnitSelector::Kind(crate::SOURCE_FILE_UNIT_KIND),
            UnitOrder::SourceOrder,
            Pipeline::new("frontend.installed_source_archive").pass(passes::ParseSourceFilePass),
        )
        .pass(super::restore::LoadLibraryFactsPass(Some(image), true))
        .pass(passes::BuildModuleCatalogPass)
        .pass(passes::ValidateExternalEnvironmentPass)
        .pass(passes::RetainDefinitionSourcesPass)
        .pass(crate::code::pipeline::passes::ResolveReflectionTargetsPass)
        .pass(emit);
    let run = crate::pipeline::run_controlled_context_pipeline(
        ProjectContext::new_with_std_registry(input, registry),
        pipeline,
        profile.is_enabled(),
        None,
        None,
    );
    for record in &run.records {
        if let Some(timing) = &record.timing {
            profile.counter(
                format!("std.installed.pass.{}.duration_ns", record.pass),
                timing.duration.as_nanos().min(u128::from(u64::MAX)) as u64,
            );
        }
    }
    if let PassControl::Failed(failure) = run.control {
        return Err(error(failure.message));
    }
    run.output.checked.ok_or_else(|| {
        error(format!(
            "installed library publication failed: {:?}",
            run.output.diagnostics
        ))
    })
}

struct PublishInstalledLibraryPass {
    semantic_dependencies: Arc<crate::project::CheckedSemanticDependencies>,
    publication: InstalledPublication,
    effects: Arc<etas_effects::EffectFacts>,
    registry: Arc<etas_effects::EffectRegistry>,
    concurrency: Arc<crate::ConcurrencyFacts>,
    cpu_parallel: Arc<crate::CpuParallelEligibilityFacts>,
    instantiations: Arc<crate::project::instantiation::NativeInstantiationPlan>,
}

impl Pass<ProjectContext> for PublishInstalledLibraryPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PublishInstalledLibraryPass", PassKind::Emit)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                MODULE_INDEX,
                RESOLVED_IMPORTS,
                TOP_LEVEL_LET_FACTS,
                PROJECT_ENTRY,
                REACHABILITY_FACTS,
                CHECKED_REFLECTION_TARGETS,
                DEFINITION_SOURCES,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::one(CHECKED_PROJECT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context
            .diagnostics
            .iter()
            .any(|d| d.severity == etas_core::Severity::Error)
        {
            return PassResult::failed(format!(
                "installed library source association failed: {}",
                context
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
                    .map(|diagnostic| format!(
                        "{} (source {})",
                        diagnostic.message, diagnostic.primary.span.source.0
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let types = context.types.as_ref().expect("installed type facts");
        let environment = context
            .validated_external_environment
            .as_ref()
            .expect("installed environment");
        let entry = context.entry.as_ref().expect("installed entry");
        let data = CheckedProgramData {
            definition_catalog_cache: Arc::default(),
            semantic_dependencies: self.semantic_dependencies.clone(),
            reflection_targets: context
                .reflection_targets
                .clone()
                .expect("installed reflection owner"),
            foreign_type_bindings: Arc::default(),
            native_instantiations: self.instantiations.clone(),
            compiler_version: crate::FRONTEND_COMPILER_VERSION.to_owned(),
            std_registry: context.std_registry.clone(),
            standard_library: None,
            toolchain_base: None,
            checked_std_library_implementations: Arc::new(self.publication.implementations.clone()),
            project_environment_fingerprint: environment.fingerprint().to_owned(),
            dependency_metadata_fingerprints: environment
                .environment()
                .canonical_dependency_metadata_fingerprints(),
            sources: context
                .sources
                .as_ref()
                .expect("installed sources")
                .to_source_bundle(),
            module_index: context.modules.clone().expect("installed modules"),
            hir_item_bindings: Arc::new(
                context
                    .hir_item_bindings
                    .clone()
                    .expect("installed item bindings"),
            ),
            resolved_imports: context.resolved_imports.clone().expect("installed imports"),
            definition_sources: context
                .definition_sources
                .clone()
                .expect("installed native sources"),
            definition_environment: environment.clone(),
            frozen_agent_call_boundaries: Default::default(),
            compile_options: context.input.options.clone(),
            optimization_report: context.optimization_report.clone(),
            hir: context.hir.as_ref().expect("installed HIR").hir.clone(),
            top_level_lets: context
                .top_level_lets
                .clone()
                .expect("installed top-level bindings"),
            types: types.facts.clone(),
            type_store: types.store.clone(),
            effects: self.effects.clone(),
            concurrency: self.concurrency.clone(),
            cpu_parallel: self.cpu_parallel.clone(),
            effect_registry: self.registry.clone(),
            external_tool_schemas: environment
                .environment()
                .external_public_metadata
                .iter()
                .flat_map(|metadata| metadata.tool_schemas.iter().cloned())
                .collect(),
            entry_fact: entry.clone(),
            entry: entry.resolved.as_ref().map(|entry| entry.item),
            reachability: context
                .reachability
                .clone()
                .expect("installed reachability"),
            residual: Arc::new(crate::residual::CheckedResidualPlans::empty_for_target(
                context.input.options.residual_target.clone(),
            )),
        };
        context.checked = Some(Arc::new(data));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}
