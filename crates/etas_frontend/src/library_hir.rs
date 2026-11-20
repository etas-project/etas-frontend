//! Portable HIR component for library bundles. This preserves the checked
//! graph's scopes, private declarations, type/effect facts, generic applications
//! and source map. Portable executable loading rebuilds native provenance and
//! passes the normal publication gates. An authenticated toolchain installation
//! can reuse finalized static facts and bind only process-local source and
//! reflection owners.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use etas_code::{DefinitionKey, MethodId, PackageKey, StableSourceKey};
use etas_core::SourceId;
use etas_hir::{HirItemId, HirProgram, HirRevision};
use serde::{Deserialize, Serialize};

use crate::{
    CheckedProgramData,
    code::identity::{DefinitionIdentityIndex, DefinitionOrigins, standard_package_key},
};

mod installed;
mod modules;
pub use modules::LibraryModuleCompilation;
mod restore;

const SCHEMA: u32 = 70;
const EFFECT_ANALYSIS_SCHEMA: u32 = 1;
const PREPARED_EFFECT_MAGIC: &[u8; 8] = b"ETASEFF2";

/// Input to fact materialization, before runtime proofs are projected and bound.
#[derive(Serialize, Deserialize)]
pub(crate) struct ArchivedEffectAnalysis {
    pub analysis: Arc<etas_effects::EffectAnalysisOutput>,
    // Effect solving can materialize types or refine task rows. Reproduce that
    // input revision if it differs from the one loaded from the body archive.
    pub types: Option<ArchivedEffectTypes>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ArchivedEffectTypes {
    pub facts: Arc<etas_types::TypeFacts>,
    pub store: Arc<etas_types::TypeStore>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PreparedLibraryEffects {
    pub effects: etas_effects::EffectOutput,
    pub types: Option<ArchivedEffectTypes>,
}

#[derive(Clone)]
pub(crate) enum LibraryEffectAnalysisMode {
    Capture(Arc<OnceLock<PreparedLibraryEffects>>),
    Replay(Arc<ArchivedEffectAnalysis>),
    Prepared(Arc<PreparedLibraryEffects>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryHirSource {
    pub id: SourceId,
    pub key: StableSourceKey,
    pub relative_path: PathBuf,
    pub text: String,
    pub kind: crate::SourceKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryHirImage {
    pub schema: u32,
    pub compiler_version: String,
    pub package: PackageKey,
    pub declarations: BTreeMap<DefinitionKey, HirItemId>,
    pub methods: BTreeMap<MethodId, HirItemId>,
    pub parameter_defaults: BTreeMap<
        etas_code::descriptor::ParameterDefaultInfo,
        crate::code::parameter_defaults::ParameterDefaultUnit,
    >,
    pub sources: Vec<LibraryHirSource>,
    pub program: Arc<HirProgram>,
    pub type_store: Arc<etas_types::TypeStore>,
    pub types: Arc<etas_types::TypeFacts>,
    pub effects: Arc<etas_effects::EffectFacts>,
    pub effect_registry: Arc<etas_effects::EffectRegistry>,
    pub concurrency: Arc<crate::ConcurrencyFacts>,
    pub cpu_parallel: Arc<crate::CpuParallelEligibilityFacts>,
    pub(crate) native_instantiations: Arc<crate::project::instantiation::NativeInstantiationPlan>,
    pub std_contract: String,
    pub native_registry: etas_std::StdRegistryImage,
    pub environment: crate::ProjectEnvironmentInput,
    pub compile_options: crate::ProjectCompileOptions,
    pub optimization_report: crate::HirOptimizationReport,
    pub module_index: crate::ModuleIndex,
    pub resolved_imports: crate::ResolvedImports,
    pub(crate) item_sources: BTreeMap<HirItemId, crate::AstItemRef>,
    pub(crate) item_targets: Vec<(crate::AstItemRef, HirItemId)>,
    pub(crate) external_import_members:
        BTreeMap<etas_hir::SymbolId, crate::project::HirExternalImportMember>,
    pub(crate) body_slots: BTreeMap<HirItemId, crate::AstBodyRef>,
    pub reflection_paths: crate::ResolvedPaths,
    pub top_level_lets: crate::TopLevelLetFacts,
    pub entry: crate::ProjectEntryFact,
    pub reachability: crate::ReachabilityFacts,
}

struct LibraryDefinitions {
    declarations: BTreeMap<DefinitionKey, HirItemId>,
    methods: BTreeMap<MethodId, HirItemId>,
    parameter_defaults: BTreeMap<
        etas_code::descriptor::ParameterDefaultInfo,
        crate::code::parameter_defaults::ParameterDefaultUnit,
    >,
}

impl LibraryDefinitions {
    fn from_checked(checked: &CheckedProgramData) -> Result<Self, LibraryHirImageError> {
        if !checked.frozen_agent_call_boundaries.is_empty()
            || !checked.foreign_type_bindings.is_empty()
        {
            return Err(error(
                "library bundles must be built from source, without live candidate bindings",
            ));
        }
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::checked_source_modules(checked).map_err(error)?,
            standard_package: standard_package_key(&checked.std_registry).map_err(error)?,
            external_definitions: DefinitionOrigins::checked_external_definitions(checked)
                .map_err(error)?,
        };
        let identities = DefinitionIdentityIndex::build(checked, &origins).map_err(error)?;
        let parameter_defaults =
            crate::code::parameter_defaults::collect_units(checked.into(), &identities)
                .map_err(error)?;
        let mut declarations = BTreeMap::new();
        let mut methods = BTreeMap::new();
        for (item, declaration) in checked.hir.items.iter() {
            let Some(symbol) = declaration.symbol() else {
                continue;
            };
            if let Some(method) = identities.method(symbol) {
                methods.insert(method.clone(), item);
            } else if let Some(key) = identities.symbol(symbol) {
                declarations.insert(key.clone(), item);
            }
        }
        Ok(Self {
            declarations,
            methods,
            parameter_defaults,
        })
    }
}

impl LibraryHirImage {
    pub fn from_checked(
        checked: &CheckedProgramData,
        package: PackageKey,
        package_root: &Path,
    ) -> Result<Self, LibraryHirImageError> {
        let LibraryDefinitions {
            declarations,
            methods,
            parameter_defaults,
        } = LibraryDefinitions::from_checked(checked)?;
        let sources = checked
            .sources
            .sources
            .iter()
            .map(|source| {
                let path = source
                    .path
                    .as_deref()
                    .ok_or_else(|| error("library source has no path"))?;
                let relative_path = path.strip_prefix(package_root).map_err(error)?.to_owned();
                Ok(LibraryHirSource {
                    id: source.id,
                    key: checked
                        .sources
                        .stable_source_key(source.id)
                        .map_err(|e| error(format!("{e:?}")))?,
                    relative_path,
                    text: source.text().to_owned(),
                    kind: checked
                        .definition_sources
                        .source(source.id)
                        .ok_or_else(|| error("library source is missing its native archive"))?
                        .kind
                        .clone(),
                })
            })
            .collect::<Result<Vec<_>, LibraryHirImageError>>()?;
        let mut item_targets = checked
            .hir_item_bindings
            .ast_to_hir
            .iter()
            .map(|(source, item)| Ok((encode_value(source)?, source.clone(), *item)))
            .collect::<Result<Vec<_>, LibraryHirImageError>>()?;
        item_targets.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        Ok(Self {
            schema: SCHEMA,
            compiler_version: checked.compiler_version.clone(),
            package,
            declarations,
            methods,
            parameter_defaults,
            sources,
            program: checked.hir.shared_program().clone(),
            type_store: checked.type_store.clone(),
            types: checked.types.clone(),
            effects: checked.effects.clone(),
            effect_registry: checked.effect_registry.clone(),
            concurrency: checked.concurrency.clone(),
            cpu_parallel: checked.cpu_parallel.clone(),
            native_instantiations: checked.native_instantiations.clone(),
            std_contract: checked.std_registry.version().compatibility_identity(),
            native_registry: etas_std::StdRegistryImage::capture(&checked.std_registry)
                .map_err(error)?,
            environment: checked.definition_environment.environment().clone(),
            compile_options: checked.compile_options.clone(),
            optimization_report: checked.optimization_report.clone(),
            module_index: checked.module_index.as_ref().clone(),
            resolved_imports: checked.resolved_imports.as_ref().clone(),
            item_sources: checked
                .hir_item_bindings
                .hir_to_ast
                .iter()
                .map(|(id, source)| (*id, source.clone()))
                .collect(),
            item_targets: item_targets
                .into_iter()
                .map(|(_, source, item)| (source, item))
                .collect(),
            external_import_members: checked
                .hir_item_bindings
                .external_import_members
                .iter()
                .map(|(id, member)| (*id, member.clone()))
                .collect(),
            body_slots: checked
                .definition_sources
                .library_body_slots()
                .map_err(error)?,
            reflection_paths: checked
                .reflection_targets
                .library_paths(&checked.hir)
                .map_err(error)?,
            top_level_lets: checked.top_level_lets.clone(),
            entry: checked.entry_fact.clone(),
            reachability: checked.reachability.clone(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, LibraryHirImageError> {
        encode_value(self)
    }

    /// Native codec for a toolchain-paired installation. Portable library
    /// artifacts continue using the self-describing body codec.
    pub fn encode_prepared(&self) -> Result<Vec<u8>, LibraryHirImageError> {
        bincode::serde::encode_to_vec(self, bincode::config::standard()).map_err(error)
    }

    pub fn decode_prepared(payload: &[u8]) -> Result<Self, LibraryHirImageError> {
        let image: Self = decode_prepared_value(payload)?;
        if image.schema != SCHEMA || image.compiler_version != crate::FRONTEND_COMPILER_VERSION {
            return Err(error("incompatible prepared library HIR image"));
        }
        Ok(image)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LibraryHirImageError> {
        let mut cursor = std::io::Cursor::new(bytes);
        let image: Self = ciborium::de::from_reader(&mut cursor).map_err(error)?;
        if cursor.position() != bytes.len() as u64 {
            return Err(error("trailing bytes in library HIR image"));
        }
        if image.schema != SCHEMA || image.compiler_version != env!("CARGO_PKG_VERSION") {
            return Err(error(format!(
                "incompatible library HIR image: expected schema {SCHEMA}, compiler {}; got schema {}, compiler {}",
                env!("CARGO_PKG_VERSION"),
                image.schema,
                image.compiler_version
            )));
        }
        Ok(image)
    }

    pub fn into_revision(self) -> Result<HirRevision, LibraryHirImageError> {
        HirRevision::from_shared_program(self.program)
            .map_err(|e| error(format!("invalid library HIR graph: {e:?}")))
    }

    /// Rebuild native provenance and execution proofs around the archived typed
    /// graph. Source parsing serves reflection; bodies are never lowered or
    /// typechecked again. The caller supplies the verified toolchain registry.
    pub fn into_checked(
        self,
        package_root: &Path,
        registry: std::sync::Arc<etas_std::StdRegistry>,
    ) -> Result<crate::CheckedProgram, LibraryHirImageError> {
        restore::restore(
            self,
            package_root,
            registry,
            &etas_utils::ProfileHandle::disabled(),
            None,
        )
    }

    /// Restore using the image's validated query view, without constructing the
    /// handwritten registry or consulting it for absent declarations.
    pub fn into_checked_from_image(
        self,
        package_root: &Path,
    ) -> Result<crate::CheckedProgram, LibraryHirImageError> {
        self.into_checked_from_image_profiled(package_root, &etas_utils::ProfileHandle::disabled())
    }

    pub fn into_checked_from_image_profiled(
        self,
        package_root: &Path,
        profile: &etas_utils::ProfileHandle,
    ) -> Result<crate::CheckedProgram, LibraryHirImageError> {
        self.restore_with_analysis(package_root, profile, None)
    }

    /// Verify the archived program during installation and retain materialized
    /// Effect facts, including suspension analysis, for this exact HIR image.
    pub fn capture_effect_analysis(
        &self,
        package_root: &Path,
        profile: &etas_utils::ProfileHandle,
    ) -> Result<Vec<u8>, LibraryHirImageError> {
        let capture = Arc::new(OnceLock::new());
        self.clone().restore_with_analysis(
            package_root,
            profile,
            Some(LibraryEffectAnalysisMode::Capture(capture.clone())),
        )?;
        let analysis = capture
            .get()
            .ok_or_else(|| error("installed library has no solved Effect analysis"))?;
        let mut bytes = PREPARED_EFFECT_MAGIC.to_vec();
        bytes.extend(
            bincode::serde::encode_to_vec(analysis, bincode::config::standard()).map_err(error)?,
        );
        Ok(bytes)
    }

    /// Prepare direct publication from the finalized checked program. Programs
    /// with residual monitors keep the full proof-rebinding path instead.
    pub fn prepare_installed_publication(
        &self,
        checked: &CheckedProgramData,
        package_root: &Path,
        profile: &etas_utils::ProfileHandle,
    ) -> Result<Vec<u8>, LibraryHirImageError> {
        if !checked.residual.is_empty() {
            return self.capture_effect_analysis(package_root, profile);
        }
        let payload = installed::encode(checked)?;
        let published =
            self.clone()
                .into_checked_from_installed_analysis(package_root, &payload, profile)?;
        let definitions = LibraryDefinitions::from_checked(&published)?;
        if definitions.declarations != self.declarations
            || definitions.methods != self.methods
            || definitions.parameter_defaults != self.parameter_defaults
        {
            return Err(error(
                "installed publication changed the library definition index",
            ));
        }
        Ok(payload)
    }

    /// Reuse solved analysis authenticated by the toolchain installation. The
    /// caller must verify that the bytes belong to this exact image and to the
    /// running toolchain before invoking this method.
    pub fn into_checked_from_installed_analysis(
        self,
        package_root: &Path,
        payload: &[u8],
        profile: &etas_utils::ProfileHandle,
    ) -> Result<crate::CheckedProgram, LibraryHirImageError> {
        if let Some(payload) = payload.strip_prefix(installed::MAGIC) {
            return installed::load(self, package_root, payload, profile);
        }
        if let Some(payload) = payload.strip_prefix(PREPARED_EFFECT_MAGIC) {
            let prepared: PreparedLibraryEffects = {
                let _decode = profile.span("std.decode_effect_analysis", "std");
                decode_prepared_value(payload)?
            };
            return self.restore_with_analysis(
                package_root,
                profile,
                Some(LibraryEffectAnalysisMode::Prepared(Arc::new(prepared))),
            );
        }
        let (schema, analysis): (u32, ArchivedEffectAnalysis) = {
            let _decode = profile.span("std.decode_effect_analysis", "std");
            let mut cursor = std::io::Cursor::new(payload);
            let analysis = ciborium::de::from_reader(&mut cursor).map_err(error)?;
            if cursor.position() != payload.len() as u64 {
                return Err(error("trailing bytes in library Effect analysis"));
            }
            analysis
        };
        if schema != EFFECT_ANALYSIS_SCHEMA {
            return Err(error("incompatible library Effect analysis schema"));
        }
        self.restore_with_analysis(
            package_root,
            profile,
            Some(LibraryEffectAnalysisMode::Replay(Arc::new(analysis))),
        )
    }

    fn restore_with_analysis(
        self,
        package_root: &Path,
        profile: &etas_utils::ProfileHandle,
        analysis: Option<LibraryEffectAnalysisMode>,
    ) -> Result<crate::CheckedProgram, LibraryHirImageError> {
        let registry = Arc::new(
            self.native_registry
                .clone()
                .into_registry()
                .map_err(error)?,
        );
        restore::restore(self, package_root, registry, profile, analysis)
    }
}

#[derive(Debug)]
pub struct LibraryHirImageError(String);

impl std::fmt::Display for LibraryHirImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for LibraryHirImageError {}

fn error(value: impl std::fmt::Display) -> LibraryHirImageError {
    LibraryHirImageError(value.to_string())
}

fn decode_prepared_value<T: serde::de::DeserializeOwned>(
    payload: &[u8],
) -> Result<T, LibraryHirImageError> {
    let (value, consumed) = bincode::serde::decode_from_slice(
        payload,
        bincode::config::standard().with_limit::<{ 512 * 1024 * 1024 }>(),
    )
    .map_err(error)?;
    if consumed != payload.len() {
        return Err(error("trailing bytes in prepared library data"));
    }
    Ok(value)
}

fn encode_value(value: &impl Serialize) -> Result<Vec<u8>, LibraryHirImageError> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).map_err(error)?;
    Ok(bytes)
}
