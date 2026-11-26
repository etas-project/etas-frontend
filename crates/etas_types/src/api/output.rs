use etas_core::Diagnostic;
use std::sync::Arc;

use crate::{TypeFacts, TypeStore};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeOutput {
    pub facts: Arc<TypeFacts>,
    pub store: Arc<TypeStore>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TypeOutput {
    /// Let checked substitutions register structural types while the consumer
    /// reads the program. Commit their IDs and runtime representations together.
    pub fn with_specializations<R>(
        &mut self,
        limit: usize,
        analyze: impl FnOnce(&Self) -> R,
    ) -> Result<R, crate::TypeSubstitutionError> {
        let original = self.store.clone();
        let first = original.len();
        Arc::make_mut(&mut self.store).begin_specializations(limit);
        let result = analyze(self);
        // Keep the old checked snapshot if there were no derived types or if
        // preparing their runtime representations fails.
        let active = std::mem::replace(&mut self.store, original);
        let end = active.len();
        if end > first {
            let mut store = Arc::unwrap_or_clone(active);
            store.finish_specializations();
            let mut interner = crate::TypeInterner::from_store(store);
            crate::ty::materialize_representations(
                &mut interner,
                (first..end).map(|index| crate::TypeId(index as u32)),
            )?;
            self.store = Arc::new(interner.into_store());
        }
        Ok(result)
    }
}
