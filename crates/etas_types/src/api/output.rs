use etas_core::Diagnostic;

use crate::{TypeFacts, TypeStore};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TypeOutput {
    pub facts: TypeFacts,
    pub store: TypeStore,
    pub diagnostics: Vec<Diagnostic>,
}
