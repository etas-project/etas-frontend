use std::sync::Arc;

use etas_core::Diagnostic;
use etas_hir::{HirItemId, HirProgram};

use crate::ty::{SharedTypeBase, TypeBaseId};
use crate::{SpecFacts, Type, TypeFacts, TypeOutput, pipeline::symbols::TypeSymbolIndex};

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
