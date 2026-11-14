//! Portable HIR component for library bundles. This preserves the checked
//! graph's scopes, private declarations and source map. Loading it produces HIR,
//! not a CheckedProgram; a bundle must also carry its matching type/effect and
//! binding artifacts before it can be linked for execution.

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

const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryHirSource {
    pub id: SourceId,
    pub key: StableSourceKey,
    pub relative_path: PathBuf,
    pub text: String,
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
}

impl LibraryHirImage {
    pub fn from_checked(
        checked: &CheckedProgramData,
        package: PackageKey,
        package_root: &Path,
    ) -> Result<Self, LibraryHirImageError> {
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
                })
            })
            .collect::<Result<Vec<_>, LibraryHirImageError>>()?;
        Ok(Self {
            schema: SCHEMA,
            compiler_version: checked.compiler_version.clone(),
            package,
            declarations,
            methods,
            sources,
            program: checked.hir.program().clone(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, LibraryHirImageError> {
        bincode::serde::encode_to_vec(self, bincode::config::standard()).map_err(error)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LibraryHirImageError> {
        let (image, consumed): (Self, usize) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard()).map_err(error)?;
        if consumed != bytes.len() {
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
