use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use etas_code::{
    RevisionKey,
    definition::{DefinitionOperationError, DefinitionRef, DefinitionResource},
};
use etas_core::memory::MemoryFootprint;

use super::{
    NativeAgentMethodView, NativeDefinition, NativeDefinitionError, NativeDefinitionLimits,
    NativeDefinitionView,
};
use crate::code::{definition::DefinitionCatalog, service::NativeCodeSyntaxOperations};

#[derive(Default)]
struct Objects {
    revisions: HashMap<RevisionKey, Vec<Weak<NativeDefinition>>>,
    source_views: HashMap<SourceReflectionKey, Weak<NativeDefinition>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum SourceReflectionKey {
    Applied {
        item: etas_hir::HirItemId,
        application: etas_code::CodeFingerprint,
        access: Option<etas_hir::HirModuleId>,
    },
    ExternalFlow {
        identity: etas_code::CallableId,
        access: Option<etas_hir::HirModuleId>,
    },
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
    pub fn has_flow_snapshot(&self, identity: &etas_code::CallableId) -> bool {
        self.catalog.item_owner(identity).is_some()
            || self
                .catalog
                .external_callables
                .get(identity)
                .is_some_and(|flow| flow.snapshot.is_some())
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
        reference: &DefinitionRef,
    ) -> Result<NativeDefinitionView, DefinitionOperationError> {
        let objects = self.lock()?;
        let object = objects
            .revisions
            .get(&reference.revision().key())
            .into_iter()
            .flatten()
            .filter_map(Weak::upgrade)
            .find(|object| reference.is_backed_by(object))
            .ok_or(DefinitionOperationError::ForeignDefinition)?;
        Ok(NativeDefinitionView(object))
    }

    pub fn methods(
        &self,
        definition: &DefinitionRef,
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
        key: &SourceReflectionKey,
    ) -> Result<Option<DefinitionRef>, DefinitionOperationError> {
        Ok(self
            .lock()?
            .source_views
            .get(key)
            .and_then(Weak::upgrade)
            .map(|object| DefinitionRef::from_backend(object)))
    }

    pub(super) fn remember_source_view(
        &self,
        key: SourceReflectionKey,
        reference: DefinitionRef,
    ) -> Result<DefinitionRef, DefinitionOperationError> {
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
                && let Some(old) = objects.source_views.keys().next().cloned()
            {
                objects.source_views.remove(&old);
            }
            objects.source_views.insert(key, Arc::downgrade(&object));
        }
        Ok(reference)
    }

    pub(super) fn publish(
        &self,
        object: NativeDefinition,
    ) -> Result<DefinitionRef, DefinitionOperationError> {
        let methods = object
            .data
            .base()
            .contract()
            .callables()
            .filter(|(id, _)| id.as_method().is_some());
        let mut expected = 0;
        for (id, _) in methods {
            expected += 1;
            if object.method_snapshots.get(id).is_none_or(|snapshot| {
                &snapshot.revision.definition != id
                    || &snapshot.identity != object.data.base().identity()
            }) {
                return Err(DefinitionOperationError::BackendUnavailable {
                    reason:
                        "definition publication is missing matching method revision coordinates"
                            .into(),
                });
            }
        }
        if object.method_snapshots.len() != expected {
            return Err(DefinitionOperationError::BackendUnavailable {
                reason: "definition publication contains foreign method revision coordinates"
                    .into(),
            });
        }
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
                    && same_native_application(&existing.environment, &object.environment)
                    && existing.data.body().map(|body| body.identity())
                        == object.data.body().map(|body| body.identity())
            })
        {
            return Ok(DefinitionRef::from_backend(existing));
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
        Ok(DefinitionRef::from_backend(object))
    }
}

fn same_native_application(
    left: &super::DefinitionEnvironment,
    right: &super::DefinitionEnvironment,
) -> bool {
    match (left, right) {
        (
            super::DefinitionEnvironment::Native(left),
            super::DefinitionEnvironment::Native(right),
        ) => match (left.callable_application(), right.callable_application()) {
            (Some(left), Some(right)) => same_checked_application(left, right),
            (None, None) => true,
            _ => false,
        },
        (super::DefinitionEnvironment::Metadata(_), super::DefinitionEnvironment::Metadata(_)) => {
            true
        }
        _ => false,
    }
}

pub(super) fn same_checked_application(
    left: &crate::CheckedCallableApplication,
    right: &crate::CheckedCallableApplication,
) -> bool {
    Arc::ptr_eq(left.program(), right.program())
        && left.owner() == right.owner()
        && left.arguments() == right.arguments()
}

fn retained_bytes(objects: &Vec<Arc<NativeDefinition>>) -> u64 {
    MemoryFootprint::of(objects)
        .estimated_bytes()
        .saturating_sub((objects.capacity() * size_of::<Arc<NativeDefinition>>()) as u64)
}

#[cfg(test)]
mod tests {
    use super::{NativeDefinitionStore, SourceReflectionKey, same_checked_application};
    use crate::{
        EntryPolicy, Frontend, ProjectInput, SourceInput,
        code::{
            definition::DefinitionCatalog, identity::DefinitionOrigins,
            service::NativeCodeSyntaxOperations,
        },
    };
    use etas_code::{CodeFingerprint, PackageKey, descriptor::DescriptorLimits};
    use etas_types::{PrimitiveType, Type};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn cache_hit_requires_the_full_sealed_application_not_just_the_index() {
        let prepare = || {
            let mut input = ProjectInput::single_source(SourceInput::anonymous(
                "module app; flow phantom<T>() -> bool ![] { return true; } flow inspect(selected: () -> bool ![]) -> unit { let definition = ^selected; return; } flow main() -> unit { phantom<bool>(); phantom<i32>(); return; }",
            ));
            input.options.entry_policy = EntryPolicy::Optional;
            let output = Frontend.check_project(input);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            output.checked.unwrap()
        };
        let application = |checked: &crate::CheckedProgram, primitive| {
            let owner = checked
                .items
                .iter()
                .find_map(|(id, item)| match item {
                    etas_hir::HirItem::Flow(flow)
                        if checked.symbols.get(flow.symbol).unwrap().name == "phantom" =>
                    {
                        Some(id)
                    }
                    _ => None,
                })
                .unwrap();
            let ty = checked
                .type_store
                .iter()
                .find_map(|(id, ty)| (ty == &Type::Primitive(primitive)).then_some(id))
                .unwrap();
            checked
                .checked_callable_application(owner, &[("T".into(), ty)], &[])
                .unwrap()
        };
        let checked = prepare();
        let other_program = prepare();
        let first = application(&checked, PrimitiveType::Bool);
        let same = application(&checked, PrimitiveType::Bool);
        let other_binding = application(&checked, PrimitiveType::I32);
        let foreign = application(&other_program, PrimitiveType::Bool);
        let applications = checked
            .checked_callable_applications(first.owner())
            .unwrap();
        struct Cancel;
        impl etas_utils::PipelineExecutionControl for Cancel {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel application enumeration".into(),
                })
            }
        }
        assert!(matches!(
            checked.checked_callable_applications_controlled(first.owner(), &Cancel),
            Err(crate::CallableApplicationError::Interrupted(
                etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    ..
                }
            ))
        ));
        struct CancelDuring(std::sync::atomic::AtomicUsize);
        impl etas_utils::PipelineExecutionControl for CancelDuring {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2 {
                    Err(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::Cancelled,
                        reason: "cancel during application enumeration".into(),
                    })
                } else {
                    Ok(())
                }
            }
        }
        assert!(matches!(
            checked.checked_callable_applications_controlled(
                first.owner(),
                &CancelDuring(std::sync::atomic::AtomicUsize::new(0))
            ),
            Err(crate::CallableApplicationError::Interrupted(
                etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    ..
                }
            ))
        ));
        assert_eq!(applications.len(), 2);
        assert!(
            applications
                .iter()
                .any(|item| same_checked_application(item, &first))
        );
        assert!(
            applications
                .iter()
                .any(|item| same_checked_application(item, &other_binding))
        );
        assert!(
            !applications
                .iter()
                .any(|item| same_checked_application(item, &foreign))
        );
        assert_eq!(first.signature(), other_binding.signature());
        assert!(same_checked_application(&first, &same));
        assert!(!same_checked_application(&first, &other_binding));
        assert!(!same_checked_application(&first, &foreign));

        let package = |name: &[u8]| {
            PackageKey::from_canonical_fingerprint(CodeFingerprint::of_bytes(
                "test.reflection-collision.v1",
                name,
            ))
        };
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(&checked, package(b"app"))
                .unwrap(),
            standard_package: package(b"std"),
            external_definitions: BTreeMap::new(),
        };
        let catalog = Arc::new(
            DefinitionCatalog::build(checked.clone(), &origins, DescriptorLimits::default())
                .unwrap(),
        );
        let store = NativeDefinitionStore::new(
            catalog,
            Arc::new(NativeCodeSyntaxOperations::default()),
            Default::default(),
        );
        let expr = *checked.types.reflections.keys().next().unwrap();
        let original = store
            .reflect_callable_application(expr, &first, DescriptorLimits::default())
            .unwrap();
        let other = store
            .reflect_callable_application(expr, &other_binding, DescriptorLimits::default())
            .unwrap();
        let other_revision = other.revision().key();
        drop(other);
        let index = SourceReflectionKey::Applied {
            item: other_binding.owner(),
            application: store
                .catalog
                .callable_application_fingerprint(&other_binding, DescriptorLimits::default())
                .unwrap(),
            access: checked.types.reflections[&expr].access_module,
        };
        {
            // Simulate both index and revision digest collision by placing the
            // other application's live object in these acceleration buckets.
            let mut objects = store.lock().unwrap();
            let object = objects.revisions[&original.revision().key()][0]
                .upgrade()
                .unwrap();
            objects.source_views.insert(index, Arc::downgrade(&object));
            objects
                .revisions
                .insert(other_revision, vec![Arc::downgrade(&object)]);
        }
        let recovered = store
            .reflect_callable_application(expr, &other_binding, DescriptorLimits::default())
            .unwrap();
        assert!(!recovered.is_same_object(&original));
        let view = store.resolve(&recovered).unwrap();
        assert!(same_checked_application(
            view.environment().unwrap().callable_application().unwrap(),
            &other_binding
        ));
    }
}
