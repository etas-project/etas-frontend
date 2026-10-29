use std::sync::Arc;

use etas_core::Diagnostic;
use etas_hir::{HirItemId, HirProgram};

use crate::ty::{SharedTypeBase, TypeBaseId};
use crate::{
    SpecFacts, Type, TypeFacts, TypeOutput, TypeStore, pipeline::symbols::TypeSymbolIndex,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureBaseExtensionError {
    ForeignBase,
    TypeLimit { limit: usize },
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for SignatureBaseExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForeignBase => f.write_str("extended types do not belong to this signature base"),
            Self::TypeLimit { limit } => {
                write!(f, "extended type base exceeds its {limit} type limit")
            }
            Self::Interrupted(interruption) => write!(
                f,
                "extended type base construction interrupted: {}",
                interruption.reason
            ),
        }
    }
}

impl std::error::Error for SignatureBaseExtensionError {}

/// Prepared once per signature revision, shared by all body checks.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct SignatureEnvironment {
    pub(crate) types: SharedTypeBase,
    pub(crate) facts: Arc<TypeFacts>,
    pub(crate) specs: Arc<SpecFacts>,
    pub(crate) symbols: Arc<TypeSymbolIndex>,
    pub(crate) diagnostics: Arc<[Diagnostic]>,
}

impl SignatureEnvironment {
    pub fn new(hir: &HirProgram, signatures: &TypeOutput) -> Self {
        Self {
            types: SharedTypeBase::new((*signatures.store).clone()),
            facts: signatures.facts.clone(),
            specs: Arc::new(SpecFacts::from(signatures.facts.as_ref())),
            symbols: Arc::new(TypeSymbolIndex::build(hir)),
            diagnostics: signatures.diagnostics.clone().into(),
        }
    }

    pub fn type_base(&self) -> &SharedTypeBase {
        &self.types
    }

    pub fn owns_signature_facts(&self, facts: &Arc<TypeFacts>) -> bool {
        Arc::ptr_eq(&self.facts, facts)
    }

    /// Freeze appended types from this exact base with cooperative cancellation
    /// and a type-count limit. Byte and node budgets remain the caller's responsibility.
    pub fn extend_type_base_controlled(
        &self,
        extended: TypeStore,
        max_types: usize,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<Self, SignatureBaseExtensionError> {
        control
            .checkpoint()
            .map_err(SignatureBaseExtensionError::Interrupted)?;
        if !extended.base().is_some_and(|base| base.ptr_eq(&self.types)) {
            return Err(SignatureBaseExtensionError::ForeignBase);
        }
        if extended.len() > max_types {
            return Err(SignatureBaseExtensionError::TypeLimit { limit: max_types });
        }
        if extended.len() == self.types.len() {
            return Ok(self.clone());
        }
        let types =
            SharedTypeBase::new_controlled(extended, max_types, control).map_err(|error| {
                match error {
                    crate::ty::TypeBaseBuildError::TypeLimit { limit } => {
                        SignatureBaseExtensionError::TypeLimit { limit }
                    }
                    crate::ty::TypeBaseBuildError::Interrupted(interruption) => {
                        SignatureBaseExtensionError::Interrupted(interruption)
                    }
                }
            })?;
        Ok(Self {
            types,
            facts: self.facts.clone(),
            specs: self.specs.clone(),
            symbols: self.symbols.clone(),
            diagnostics: self.diagnostics.clone(),
        })
    }

    pub fn accepts(&self, body: &BodyTypeOutput) -> bool {
        body.base == self.types.identity() && body.base_len == self.types.len()
    }
}

/// Only body-owned facts and newly interned types. Type IDs below base_len
/// refer to the identified immutable signature universe, never an embedded copy.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct BodyTypeOutput {
    pub owner: HirItemId,
    pub base: TypeBaseId,
    pub base_len: usize,
    pub types: Vec<Type>,
    pub facts: TypeFacts,
    pub diagnostics: Vec<Diagnostic>,
}
