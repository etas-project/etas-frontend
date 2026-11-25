//! Authenticated installed facts with a lazy diagnostic/reflection source archive.
use super::{LibraryHirImage, LibraryHirImageError, decode_prepared_value, error};
use crate::{
    CheckedProgramData,
    code::{
        definition::DefinitionSources,
        identity::{DefinitionOrigins, SourceModuleOrigin},
    },
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, sync::Arc, time::Instant};

pub(super) const MAGIC: &[u8; 8] = b"ETASPUB3";

#[derive(Serialize, Deserialize)]
struct InstalledPublication {
    implementations: crate::passes::CheckedStandardLibraryImplementations,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    origins: HashMap<etas_hir::HirModuleId, SourceModuleOrigin>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    exposure: HashMap<etas_hir::HirItemId, crate::code::definition::BodyExposure>,
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
        origins: DefinitionOrigins::checked_source_modules(checked).map_err(error)?,
        exposure: checked.definition_sources.installed_exposure(),
    };
    let mut bytes = MAGIC.to_vec();
    bytes.extend(
        bincode::serde::encode_to_vec(&publication, bincode::config::standard()).map_err(error)?,
    );
    Ok(bytes)
}

pub(super) fn load(
    image: LibraryHirImage,
    root: &Path,
    payload: &[u8],
    profile: &etas_utils::ProfileHandle,
) -> Result<crate::CheckedProgram, LibraryHirImageError> {
    let _load = profile.span("std.load_installed_program", "std");
    let started = Instant::now();
    let publication: InstalledPublication = decode_prepared_value(payload)?;
    if image.schema != super::SCHEMA || image.compiler_version != crate::FRONTEND_COMPILER_VERSION {
        return Err(error("incompatible installed library image"));
    }
    let registry = Arc::new(image.native_registry.into_registry().map_err(error)?);
    if image.std_contract != registry.version().compatibility_identity() {
        return Err(error(
            "installed library and toolchain standard contracts differ",
        ));
    }
    let environment = Arc::new(crate::ValidatedExternalEnvironment::new(image.environment));
    let sources = Arc::new(crate::SourceSet {
        project_root: root.to_owned(),
        source_root: root.to_owned(),
        environment_fingerprint: environment.fingerprint().to_owned(),
        external_modules_fingerprint: environment
            .environment()
            .canonical_external_modules_fingerprint(),
        stable_source_keys: environment.environment().stable_source_keys.clone(),
        files: image
            .sources
            .into_iter()
            .map(|source| {
                crate::SourceFile::from_input(&crate::SourceInput {
                    id: source.id,
                    path: Some(root.join(source.relative_path)),
                    text: source.text,
                    kind: source.kind,
                })
            })
            .collect(),
    });
    let archive = DefinitionSources::installed(
        sources.clone(),
        image.body_slots.into_iter().collect(),
        publication.exposure,
        publication.origins,
        profile.clone(),
    );
    let hir = etas_hir::HirRevision::from_shared_program(image.program)
        .map_err(|e| error(format!("invalid installed HIR: {e:?}")))?;
    let types = etas_types::TypeOutput {
        store: image.type_store,
        facts: image.types,
        diagnostics: Vec::new(),
    };
    let reflection_targets = crate::code::reflection::CheckedReflectionTargets::resolve(
        &hir,
        &image.reflection_paths,
        &types,
        &image.reachability.analysis_items,
        false,
        None,
    )
    .map_err(|e| match e {
        crate::code::reflection::ReflectionTargetError::Invalid(reason) => error(reason),
        crate::code::reflection::ReflectionTargetError::Interrupted(interruption) => {
            error(interruption.reason)
        }
    })?;
    let data = CheckedProgramData {
        definition_catalog_cache: Arc::default(),
        semantic_dependencies: image.semantic_dependencies,
        reflection_targets: Arc::new(reflection_targets),
        foreign_type_bindings: Arc::default(),
        native_instantiations: image.native_instantiations,
        compiler_version: crate::FRONTEND_COMPILER_VERSION.to_owned(),
        std_registry: registry,
        standard_library: None,
        standard_library_interface: None,
        toolchain_base: None,
        checked_std_library_implementations: Arc::new(publication.implementations),
        project_environment_fingerprint: environment.fingerprint().to_owned(),
        dependency_metadata_fingerprints: environment
            .environment()
            .canonical_dependency_metadata_fingerprints(),
        sources: sources.to_source_bundle(),
        module_index: Arc::new(image.module_index),
        hir_item_bindings: Arc::new(crate::HirItemBindings {
            ast_to_hir: image.item_targets.into_iter().collect(),
            hir_to_ast: image.item_sources.into_iter().collect(),
            external_import_members: image.external_import_members.into_iter().collect(),
        }),
        resolved_imports: Arc::new(image.resolved_imports),
        definition_sources: Arc::new(archive),
        external_tool_schemas: environment
            .environment()
            .external_public_metadata
            .iter()
            .flat_map(|metadata| metadata.tool_schemas.iter().cloned())
            .collect(),
        definition_environment: environment,
        frozen_agent_call_boundaries: Default::default(),
        residual: Arc::new(crate::residual::CheckedResidualPlans::empty_for_target(
            image.compile_options.residual_target.clone(),
        )),
        compile_options: image.compile_options,
        optimization_report: image.optimization_report,
        hir,
        top_level_lets: image.top_level_lets,
        types: types.facts,
        type_store: types.store,
        effects: image.effects,
        concurrency: image.concurrency,
        cpu_parallel: image.cpu_parallel,
        effect_registry: image.effect_registry,
        entry: image.entry.resolved.as_ref().map(|entry| entry.item),
        entry_fact: image.entry,
        reachability: image.reachability,
    };
    profile.counter(
        "std.installed.bind.duration_ns",
        started.elapsed().as_nanos() as u64,
    );
    Ok(Arc::new(data))
}
