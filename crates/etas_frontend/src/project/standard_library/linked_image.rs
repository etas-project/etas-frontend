//! The link changes type arenas, invocation proofs and their origins. Keep those
//! changes, reattaching them to the exact caller and SDK HIR on a later run.
use super::{
    catalog,
    instantiation::{NativeInstantiation, NativeInstantiationPlan},
};
use crate::{
    CheckedProgram, CheckedProgramData, StandardLibrary,
    code::definition::{DefinitionCatalog, ForeignOrigins},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Serialize, Deserialize)]
struct Revision {
    sdk: bool,
    store: Option<usize>,
    instances: Option<Vec<NativeInstantiation>>,
    types: Option<usize>,
    effects: Option<Arc<etas_effects::EffectRegistry>>,
    origins: ForeignOrigins,
}

// These are exactly the declaration tables extended by the foreign type graph.
// Expression and HIR facts stay shared with the original checked program.
#[derive(Serialize, Deserialize)]
struct Declarations {
    generics: HashMap<etas_types::TypeId, Vec<etas_types::CallableGenericParam>>,
    enums: HashMap<etas_types::TypeId, etas_types::EnumLayoutFact>,
    canonical: etas_types::facts::CanonicalSpecFacts,
    standard: Vec<etas_types::facts::CheckedStdSpecImplFact>,
    checked: Vec<etas_types::facts::specs::CheckedSpecImplementationFact>,
    methods: Vec<etas_types::method::MethodSignatureFact>,
}

#[derive(Default, Serialize, Deserialize)]
struct LinkImage {
    stores: Vec<Arc<etas_types::TypeStore>>,
    declarations: Vec<Declarations>,
    nodes: Vec<Revision>,
    caller: usize,
    library: usize,
}

#[derive(Default)]
struct Owners {
    revisions: HashMap<*const CheckedProgramData, usize>,
    stores: HashMap<*const etas_types::TypeStore, usize>,
    declarations: HashMap<*const etas_types::TypeFacts, usize>,
}

impl LinkImage {
    fn capture(
        caller: &CheckedProgramData,
        sdk: &CheckedProgram,
        linked_caller: &CheckedProgramData,
        linked_sdk: &CheckedProgram,
    ) -> Option<Self> {
        let mut image = Self::default();
        let mut known = Owners::default();
        image.caller = image.record(&Arc::new(linked_caller.clone()), caller, sdk, &mut known)?;
        image.library = image.record(linked_sdk, caller, sdk, &mut known)?;
        Some(image)
    }
    fn record(
        &mut self,
        program: &CheckedProgram,
        caller: &CheckedProgramData,
        sdk: &CheckedProgram,
        known: &mut Owners,
    ) -> Option<usize> {
        let ptr = Arc::as_ptr(program);
        if let Some(&index) = known.revisions.get(&ptr) {
            return Some(index);
        }
        let is_sdk = Arc::ptr_eq(program.hir.shared_program(), sdk.hir.shared_program());
        if !is_sdk && !Arc::ptr_eq(program.hir.shared_program(), caller.hir.shared_program()) {
            return None;
        }
        let base = if is_sdk { sdk.as_ref() } else { caller };
        let origins = program
            .foreign_type_bindings
            .origins(|owner| self.record(owner, caller, sdk, known))?;
        let facts = &program.types;
        let types = (!Arc::ptr_eq(facts, &base.types)).then(|| {
            *known
                .declarations
                .entry(Arc::as_ptr(facts))
                .or_insert_with(|| {
                    self.declarations.push(Declarations {
                        generics: facts.type_generics.clone(),
                        enums: facts.enum_layouts.clone(),
                        canonical: facts.canonical_specs.clone(),
                        standard: facts.std_spec_impls.clone(),
                        checked: facts.checked_spec_impls.clone(),
                        methods: facts
                            .methods
                            .entries()
                            .skip(base.types.methods.entries().count())
                            .cloned()
                            .collect(),
                    });
                    self.declarations.len() - 1
                })
        });
        let store = (!Arc::ptr_eq(&program.type_store, &base.type_store)).then(|| {
            *known
                .stores
                .entry(Arc::as_ptr(&program.type_store))
                .or_insert_with(|| {
                    self.stores.push(program.type_store.clone());
                    self.stores.len() - 1
                })
        });
        let index = self.nodes.len();
        self.nodes.push(Revision {
            sdk: is_sdk,
            store,
            instances: (!Arc::ptr_eq(&program.native_instantiations, &base.native_instantiations))
                .then(|| program.native_instantiations.instances().to_vec()),
            types,
            effects: (!Arc::ptr_eq(&program.effect_registry, &base.effect_registry))
                .then(|| program.effect_registry.clone()),
            origins,
        });
        known.revisions.insert(ptr, index);
        Some(index)
    }
    fn restore(
        self,
        caller: &CheckedProgramData,
        sdk: &CheckedProgram,
    ) -> Option<(CheckedProgramData, CheckedProgram)> {
        let mut revisions: Vec<CheckedProgram> = Vec::with_capacity(self.nodes.len());
        let mut catalogs = HashMap::<usize, Arc<DefinitionCatalog>>::new();
        // HIR clones used in foreign method proofs share the exact TypeFacts
        // and TypeStore allocations with their live caller.
        let mut declarations: Vec<_> = self.declarations.into_iter().map(Some).collect();
        let mut restored = HashMap::<usize, Arc<etas_types::TypeFacts>>::new();
        for node in self.nodes {
            let mut program = if node.sdk { sdk.as_ref() } else { caller }.clone();
            if node.sdk {
                program.toolchain_base = caller.toolchain_base.clone();
            }
            if let Some(store) = node.store {
                program.type_store = self.stores.get(store)?.clone();
            }
            if let Some(instances) = node.instances {
                let mut plan = NativeInstantiationPlan::default();
                for instance in instances {
                    plan.push(instance);
                }
                program.native_instantiations = Arc::new(plan);
            }
            if let Some(id) = node.types {
                if let Some(facts) = restored.get(&id) {
                    program.types = facts.clone();
                } else {
                    let data = declarations.get_mut(id)?.take()?;
                    let mut facts = program.types.as_ref().clone();
                    facts.type_generics = data.generics;
                    facts.enum_layouts = data.enums;
                    facts.canonical_specs = data.canonical;
                    facts.std_spec_impls = data.standard;
                    facts.checked_spec_impls = data.checked;
                    if !data.methods.is_empty() {
                        facts.methods = Arc::new(
                            facts
                                .methods
                                .with_additional_declarations(&program.type_store, data.methods),
                        );
                    }
                    program.types = Arc::new(facts);
                    restored.insert(id, program.types.clone());
                }
                program.reflection_targets =
                    program.reflection_targets.with_types(program.types.clone());
            }
            if let Some(effects) = node.effects {
                program.effect_registry = effects;
            }
            program.foreign_type_bindings = Arc::new(node.origins.bind(|id| {
                if let Some(catalog) = catalogs.get(&id) {
                    return Some(catalog.clone());
                }
                let origin = Arc::new(catalog(revisions.get(id)?.clone()).ok()?);
                catalogs.insert(id, origin.clone());
                Some(origin)
            })?);
            revisions.push(Arc::new(program));
        }
        Some((
            revisions.get(self.caller)?.as_ref().clone(),
            revisions.get(self.library)?.clone(),
        ))
    }
}

impl StandardLibrary {
    pub(crate) fn link_for_execution(
        self: &Arc<Self>,
        consumer: &mut CheckedProgramData,
    ) -> Result<Arc<Self>, String> {
        let Some(cache) = &self.run_cache else {
            self.materialize_public_abis(consumer)?;
            return self.for_consumer(consumer);
        };
        {
            let _span = self.profile.span("std.consumer.restore_link", "std");
            if let Some(bytes) = cache.read_link()
                && let Ok((image, used)) = bincode::serde::decode_from_slice::<LinkImage, _>(
                    &bytes,
                    bincode::config::standard().with_limit::<536870912>(),
                )
                && used == bytes.len()
                && let Some((owner, program)) = image.restore(consumer, &self.program)
            {
                cache.bind_link(&bytes);
                *consumer = owner;
                self.profile.counter("std.consumer.link_cache_hit", 1);
                return Ok(Arc::new(Self {
                    program,
                    ..self.as_ref().clone()
                }));
            }
        }
        self.profile.counter("std.consumer.link_cache_hit", 0);
        let before = consumer.clone();
        self.materialize_public_abis(consumer)?;
        let linked = self.for_consumer(consumer)?;
        if cache.can_write() {
            let _span = self.profile.span("std.consumer.store_link", "std");
            if let Some(image) =
                LinkImage::capture(&before, &self.program, consumer, &linked.program)
                && let Ok(bytes) = bincode::serde::encode_to_vec(image, bincode::config::standard())
            {
                cache.bind_link(&bytes);
                cache.write_link(&bytes);
            }
        }
        Ok(linked)
    }
}
