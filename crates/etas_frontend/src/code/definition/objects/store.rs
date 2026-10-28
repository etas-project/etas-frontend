use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use etas_code::{
    RevisionKey,
    definition::{AgentDefinitionRef, DefinitionOperationError, DefinitionResource},
};
use etas_core::memory::MemoryFootprint;

use super::{
    NativeAgentDefinition, NativeAgentDefinitionView, NativeAgentMethodView, NativeDefinitionError,
    NativeDefinitionLimits,
};
use crate::code::{definition::DefinitionCatalog, service::NativeCodeSyntaxOperations};

#[derive(Default)]
struct Objects {
    revisions: HashMap<RevisionKey, Vec<Weak<NativeAgentDefinition>>>,
    source_views: HashMap<SourceReflectionKey, Weak<NativeAgentDefinition>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum SourceReflectionKey {
    Native {
        item: etas_hir::HirItemId,
        access: Option<etas_hir::HirModuleId>,
    },
    External {
        component: etas_types::TypeId,
        access: Option<etas_hir::HirModuleId>,
    },
}

/// Weak indexing never pins an unreferenced revision. Shared leases retain the
/// exact immutable native object; descriptor hashes are not lookup authority.
pub struct NativeDefinitionStore {
    pub(super) catalog: Arc<DefinitionCatalog>,
    pub(super) syntax: Arc<NativeCodeSyntaxOperations>,
    limits: NativeDefinitionLimits,
    objects: Mutex<Objects>,
}

impl NativeDefinitionStore {
    pub fn catalog(&self) -> &Arc<DefinitionCatalog> {
        &self.catalog
    }
    pub fn syntax_operations(&self) -> &Arc<NativeCodeSyntaxOperations> {
        &self.syntax
    }
    pub fn new(
        catalog: Arc<DefinitionCatalog>,
        syntax: Arc<NativeCodeSyntaxOperations>,
        limits: NativeDefinitionLimits,
    ) -> Self {
        Self {
            catalog,
            syntax,
            limits,
            objects: Mutex::new(Objects::default()),
        }
    }

    pub fn resolve(
        &self,
        reference: &AgentDefinitionRef,
    ) -> Result<NativeAgentDefinitionView, DefinitionOperationError> {
        let objects = self.lock()?;
        let object = objects
            .revisions
            .get(&reference.revision().key())
            .into_iter()
            .flatten()
            .filter_map(Weak::upgrade)
            .find(|object| reference.is_backed_by(object))
            .ok_or(DefinitionOperationError::ForeignDefinition)?;
        Ok(NativeAgentDefinitionView(object))
    }

    pub fn methods(
        &self,
        definition: &AgentDefinitionRef,
    ) -> Result<Vec<NativeAgentMethodView>, NativeDefinitionError> {
        self.resolve(definition)?.methods(self)
    }

    pub fn retained_bytes(&self) -> Result<u64, DefinitionOperationError> {
        let objects = self
            .lock()?
            .revisions
            .values()
            .flatten()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        Ok(retained_bytes(&objects))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Objects>, DefinitionOperationError> {
        self.objects
            .lock()
            .map_err(|_| DefinitionOperationError::BackendUnavailable {
                reason: "native definition store lock is poisoned".into(),
            })
    }

    pub(super) fn source_view(
        &self,
        key: SourceReflectionKey,
    ) -> Result<Option<AgentDefinitionRef>, DefinitionOperationError> {
        Ok(self
            .lock()?
            .source_views
            .get(&key)
            .and_then(Weak::upgrade)
            .map(|object| AgentDefinitionRef::from_backend(object)))
    }

    pub(super) fn remember_source_view(
        &self,
        key: SourceReflectionKey,
        reference: AgentDefinitionRef,
    ) -> Result<AgentDefinitionRef, DefinitionOperationError> {
        let mut objects = self.lock()?;
        let object = objects
            .revisions
            .get(&reference.revision().key())
            .into_iter()
            .flatten()
            .filter_map(Weak::upgrade)
            .find(|object| reference.is_backed_by(object))
            .ok_or(DefinitionOperationError::ForeignDefinition)?;
        objects
            .source_views
            .retain(|_, view| view.strong_count() > 0);
        // This index only accelerates immutable source lookup; leases remain
        // authoritative and its bookkeeping cannot grow with repeated requests.
        if self.limits.max_retained_objects > 0 {
            if objects.source_views.len() >= self.limits.max_retained_objects
                && let Some(old) = objects.source_views.keys().next().copied()
            {
                objects.source_views.remove(&old);
            }
            objects.source_views.insert(key, Arc::downgrade(&object));
        }
        Ok(reference)
    }

    pub(super) fn publish(
        &self,
        object: NativeAgentDefinition,
    ) -> Result<AgentDefinitionRef, DefinitionOperationError> {
        let mut objects = self.lock()?;
        objects.revisions.retain(|_, bucket| {
            bucket.retain(|entry| entry.strong_count() > 0);
            !bucket.is_empty()
        });
        let key = object.data.revision().key();
        if let Some(existing) = objects
            .revisions
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(Weak::upgrade)
            .find(|existing| {
                existing.provenance == object.provenance
                    && existing.reflection_access == object.reflection_access
                    && existing.reflection_body_access == object.reflection_body_access
                    && existing.data.body().map(|body| body.identity())
                        == object.data.body().map(|body| body.identity())
            })
        {
            return Ok(AgentDefinitionRef::from_backend(existing));
        }
        let mut live = objects
            .revisions
            .values()
            .flatten()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        if live.len() >= self.limits.max_retained_objects {
            return Err(DefinitionOperationError::ResourceLimit {
                resource: DefinitionResource::RetainedObjects,
                limit: self.limits.max_retained_objects as u64,
            });
        }
        let object = Arc::new(object);
        live.push(object.clone());
        if retained_bytes(&live) > self.limits.max_retained_bytes {
            return Err(DefinitionOperationError::ResourceLimit {
                resource: DefinitionResource::RetainedBytes,
                limit: self.limits.max_retained_bytes,
            });
        }
        objects
            .revisions
            .entry(key)
            .or_default()
            .push(Arc::downgrade(&object));
        Ok(AgentDefinitionRef::from_backend(object))
    }
}

fn retained_bytes(objects: &Vec<Arc<NativeAgentDefinition>>) -> u64 {
    MemoryFootprint::of(objects)
        .estimated_bytes()
        .saturating_sub((objects.capacity() * size_of::<Arc<NativeAgentDefinition>>()) as u64)
}
