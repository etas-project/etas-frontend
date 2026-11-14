//! Portable HIR component for library bundles. This preserves the checked
//! graph's scopes, private declarations, type/effect facts, generic applications
//! and source map. The raw graph can be loaded separately; executable loading
//! rebuilds native provenance and passes the normal publication gates.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use etas_code::{DefinitionKey, MethodId, PackageKey, StableSourceKey};
use etas_core::SourceId;
use etas_hir::{HirItemId, HirProgram, HirRevision};
use serde::{Deserialize, Serialize};

use crate::{
    CheckedProgramData,
    code::identity::{DefinitionIdentityIndex, DefinitionOrigins, standard_package_key},
};

mod restore;

const SCHEMA: u32 = 3;

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
    pub sources: Vec<LibraryHirSource>,
    pub program: HirProgram,
    pub type_store: etas_types::TypeStore,
    pub types: etas_types::TypeFacts,
    pub effects: etas_effects::EffectFacts,
    pub effect_registry: etas_effects::EffectRegistry,
    pub concurrency: crate::ConcurrencyFacts,
    pub cpu_parallel: crate::CpuParallelEligibilityFacts,
    pub(crate) native_instantiations: crate::project::instantiation::NativeInstantiationPlan,
    pub std_contract: String,
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

impl LibraryHirImage {
    pub fn from_checked(
        checked: &CheckedProgramData,
        package: PackageKey,
        package_root: &Path,
    ) -> Result<Self, LibraryHirImageError> {
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
            sources,
            program: checked.hir.program().clone(),
            type_store: checked.type_store.as_ref().clone(),
            types: checked.types.as_ref().clone(),
            effects: checked.effects.as_ref().clone(),
            effect_registry: checked.effect_registry.as_ref().clone(),
            concurrency: checked.concurrency.as_ref().clone(),
            cpu_parallel: checked.cpu_parallel.as_ref().clone(),
            native_instantiations: checked.native_instantiations.as_ref().clone(),
            std_contract: checked.std_registry.version().compatibility_identity(),
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
        HirRevision::try_new(self.program)
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
        restore::restore(self, package_root, registry)
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

fn encode_value(value: &impl Serialize) -> Result<Vec<u8>, LibraryHirImageError> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).map_err(error)?;
    Ok(bytes)
}
