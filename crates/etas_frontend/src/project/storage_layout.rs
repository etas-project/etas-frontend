//! Describes checked storage representations. This query does not grant a
//! caller access to private constructors or protected provenance.
use std::{collections::HashMap, sync::Arc};

use etas_code::{
    CodeFingerprint,
    descriptor::{DescriptorFingerprintBudget, DescriptorLimits, GenericOwner},
};
use etas_types::{
    TypeId, TypeInterner, TypeStore,
    storage_layout::{StorageIdentities, StorageIdentity, StorageLayout, StorageLayoutError},
};

use crate::{
    CheckedProgramData,
    code::{
        descriptor::{DefinitionDescriptorProjector, DescriptorInput, ProjectionError},
        identity::{DefinitionIdentityIndex, DefinitionOrigins, standard_package_key},
    },
};

pub struct StorageLayoutDescription {
    /// Includes closed logical-view types materialized while deriving adapters.
    pub types: Arc<TypeStore>,
    pub layout: StorageLayout,
}

#[derive(Debug)]
pub enum StorageLayoutDescriptionError {
    Projection(ProjectionError),
    Layout(StorageLayoutError),
}

impl std::fmt::Display for StorageLayoutDescriptionError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Projection(error) => error.fmt(out),
            Self::Layout(error) => error.fmt(out),
        }
    }
}
impl std::error::Error for StorageLayoutDescriptionError {}

impl CheckedProgramData {
    pub fn describe_storage_layout(
        &self,
        ty: TypeId,
        limits: DescriptorLimits,
    ) -> Result<StorageLayoutDescription, StorageLayoutDescriptionError> {
        let origins = (|| -> Result<_, ProjectionError> {
            let origins = DefinitionOrigins {
                source_modules: DefinitionOrigins::checked_source_modules(self)?,
                standard_package: standard_package_key(&self.std_registry)?,
                external_definitions: DefinitionOrigins::checked_external_definitions(self)?,
            };
            DefinitionIdentityIndex::build_with_limits(self, &origins, limits)
        })()
        .map_err(StorageLayoutDescriptionError::Projection)?;
        let mut identities = CheckedIdentities {
            program: self,
            index: origins,
            limits,
            arguments: HashMap::new(),
            identities: HashMap::new(),
            budget: DescriptorFingerprintBudget::new(limits.max_edges),
        };
        let mut interner = TypeInterner::from_store(self.type_store.as_ref().clone());
        let layout = StorageLayout::derive(
            &mut interner,
            &self.types,
            ty,
            &mut identities,
            limits.max_nodes,
        )
        .map_err(StorageLayoutDescriptionError::Layout)?;
        Ok(StorageLayoutDescription {
            types: Arc::new(interner.into_store()),
            layout,
        })
    }
}

struct CheckedIdentities<'a> {
    program: &'a CheckedProgramData,
    index: DefinitionIdentityIndex,
    limits: DescriptorLimits,
    arguments: HashMap<TypeId, CodeFingerprint>,
    identities: HashMap<TypeId, StorageIdentity>,
    budget: DescriptorFingerprintBudget,
}

impl StorageIdentities for CheckedIdentities<'_> {
    fn identity(&mut self, store: &TypeStore, ty: TypeId) -> Result<StorageIdentity, String> {
        if let Some(identity) = self.identities.get(&ty) {
            return Ok(identity.clone());
        }
        let (declaration, arguments) = self
            .index
            .method_subject(store, ty)
            .map_err(|error| error.to_string())?;
        let declaration = declaration.identity.clone();
        let missing = arguments
            .iter()
            .copied()
            .filter(|argument| !self.arguments.contains_key(argument))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let mut input = DescriptorInput::from(self.program);
            input.type_store = store;
            let mut projection =
                DefinitionDescriptorProjector::from_input(input, &self.index, self.limits);
            let roots = projection
                .project_closed_types(GenericOwner::Declaration(declaration.clone()), &missing)
                .map_err(|error| error.to_string())?;
            let graph = projection.finish().map_err(|error| error.to_string())?;
            for (argument, root) in missing.into_iter().zip(roots) {
                let fingerprint = graph
                    .reference_type_fingerprint(root, &mut self.budget)
                    .map_err(|error| error.to_string())?;
                self.arguments.insert(argument, fingerprint);
            }
        }
        let identity = StorageIdentity {
            declaration,
            arguments: arguments
                .into_iter()
                .map(|argument| self.arguments[&argument])
                .collect(),
        };
        self.identities.insert(ty, identity.clone());
        Ok(identity)
    }
}
