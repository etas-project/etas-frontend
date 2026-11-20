//! Revision-owned memoization. Entries contain immutable indices and weak owner
//! keys, so retaining an index never retains its checked program or a revision
//! that has been superseded. An appended type arena is a distinct owner.

use super::{DefinitionCatalog, store::DefinitionCatalogData};
use crate::code::{descriptor::ProjectionError, identity::DefinitionOrigins};
use crate::{CheckedProgram, CheckedProgramData};
use etas_code::descriptor::DescriptorLimits;
use std::sync::{Arc, OnceLock, Weak};

#[derive(Default)]
pub(crate) struct DefinitionCatalogCache {
    entry: OnceLock<Entry>,
    next: OnceLock<Arc<Self>>,
}

impl std::fmt::Debug for DefinitionCatalogCache {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("DefinitionCatalogCache")
            .finish_non_exhaustive()
    }
}

struct Owner {
    hir: Weak<etas_hir::HirProgram>,
    types: Weak<etas_types::TypeFacts>,
    store: Weak<etas_types::TypeStore>,
    effects: Weak<etas_effects::EffectFacts>,
    registry: Weak<etas_std::StdRegistry>,
    foreign: Weak<super::ForeignTypeBindings>,
    sources: Weak<super::DefinitionSources>,
    environment: Weak<crate::ValidatedExternalEnvironment>,
    standard: Option<Weak<crate::StandardLibraryInterface>>,
}

impl Owner {
    fn new(program: &CheckedProgramData) -> Self {
        Self {
            hir: Arc::downgrade(program.hir.shared_program()),
            types: Arc::downgrade(&program.types),
            store: Arc::downgrade(&program.type_store),
            effects: Arc::downgrade(&program.effects),
            registry: Arc::downgrade(&program.std_registry),
            foreign: Arc::downgrade(&program.foreign_type_bindings),
            sources: Arc::downgrade(&program.definition_sources),
            environment: Arc::downgrade(&program.definition_environment),
            standard: program
                .standard_library
                .as_ref()
                .map(|library| Arc::downgrade(&library.interface)),
        }
    }
    fn matches(&self, program: &CheckedProgramData) -> bool {
        self.hir.as_ptr() == Arc::as_ptr(program.hir.shared_program())
            && self.types.as_ptr() == Arc::as_ptr(&program.types)
            && self.store.as_ptr() == Arc::as_ptr(&program.type_store)
            && self.effects.as_ptr() == Arc::as_ptr(&program.effects)
            && self.registry.as_ptr() == Arc::as_ptr(&program.std_registry)
            && self.foreign.as_ptr() == Arc::as_ptr(&program.foreign_type_bindings)
            && self.sources.as_ptr() == Arc::as_ptr(&program.definition_sources)
            && self.environment.as_ptr() == Arc::as_ptr(&program.definition_environment)
            && self.standard.as_ref().map(Weak::as_ptr)
                == program
                    .standard_library
                    .as_ref()
                    .map(|library| Arc::as_ptr(&library.interface))
    }
}

struct Entry {
    owner: Owner,
    origins: DefinitionOrigins,
    limits: DescriptorLimits,
    data: Arc<DefinitionCatalogData>,
}

impl etas_core::memory::RetainedSize for DefinitionCatalogCache {
    fn visit_retained<'a>(&'a self, visitor: &mut etas_core::memory::MemoryVisitor<'a>) {
        if let Some(entry) = self.entry.get() {
            entry.origins.visit_retained(visitor);
            visitor.visit_shared_arc(&entry.data);
        }
        if let Some(next) = self.next.get() {
            visitor.visit_shared_arc(next);
        }
    }
}

pub(super) fn build(
    checked: CheckedProgram,
    origins: &DefinitionOrigins,
    limits: DescriptorLimits,
) -> Result<DefinitionCatalog, ProjectionError> {
    let mut cache = checked.definition_catalog_cache.clone();
    loop {
        if let Some(entry) = cache.entry.get() {
            if entry.owner.matches(&checked) && entry.origins == *origins && entry.limits == limits
            {
                return Ok(DefinitionCatalog {
                    checked,
                    data: entry.data.clone(),
                });
            }
            cache = cache.next.get_or_init(Default::default).clone();
        } else {
            let catalog = super::collect::build(checked.clone(), origins, limits)?;
            let _ = cache.entry.set(Entry {
                owner: Owner::new(&checked),
                origins: origins.clone(),
                limits,
                data: catalog.data.clone(),
            });
            return Ok(catalog);
        }
    }
}
