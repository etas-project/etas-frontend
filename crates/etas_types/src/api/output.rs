use etas_core::Diagnostic;
use std::sync::Arc;

use crate::{TypeFacts, TypeStore};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeOutput {
    pub facts: Arc<TypeFacts>,
    pub store: Arc<TypeStore>,
    pub diagnostics: Vec<Diagnostic>,
}
