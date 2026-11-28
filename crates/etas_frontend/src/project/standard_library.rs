use std::sync::Arc;

mod effects;
mod linked_image;
mod methods;
mod owner;
mod requests;

use etas_hir::SymbolDef;
use etas_types::{
    TypeInterner, TypeOutput, TypeSubstitutionEngine,
    ty::{TypeLookupIndex, TypeRelocationMaterializationInput},
};

use super::instantiation::{self, InstantiationError, NativeGenericArgument, NativeInstantiation};
use crate::{
    CheckedProgramData, StandardLibrary,
    code::{
        definition::{DefinitionCatalog, NativeTypeCorrespondence},
        identity::{DefinitionOrigins, standard_package_key},
    },
};

impl CheckedProgramData {
    pub fn checked_standard_callable_row(
        &self,
        callable: &etas_code::CallableId,
        item: etas_hir::HirItemId,
    ) -> Result<etas_types::EffectRowRef, String> {
        let signature = instantiation::signature(&self.types, item)
            .ok_or("standard library body has no checked signature")?;
        if let Some(contract) = self
            .effects
            .public_contracts
            .iter()
            .find(|contract| contract.item == item)
        {
            return instantiation::checked_row(
                &contract.public_row,
                &self.effect_registry,
                Some(signature),
                std::iter::empty(),
            );
        }
        // Private Etas reconstruction methods are available only through their
        // identified compiler ABI. Their checked, explicit row is the contract.
        if let Some(identity) = callable.as_method()
            && self.types.storage_methods().any(|(method, known)| {
                known == identity
                    && matches!(method.target,
                    etas_types::method::MethodTarget::Source { callable, .. } if callable == item)
            })
            && let Some(row) = &signature.effects
        {
            return Ok(row.clone());
        }
        Err(format!(
            "std body {callable:?} has no checked effect contract for its ABI"
        ))
    }
}

impl StandardLibrary {
    /// Function values cross the SDK boundary with the solved public effect
    /// row. Keep this ABI alongside the authored signature before execution.
    pub(crate) fn materialize_public_abis(
        &self,
        consumer: &mut CheckedProgramData,
    ) -> Result<(), String> {
        let _all = self.profile.span("std.consumer.public_abis", "std");
        let source = {
            let _phase = self.profile.span("std.consumer.body_catalog", "std");
            catalog(self.program.clone())?
        };
        let target = {
            let _phase = self.profile.span("std.consumer.caller_catalog", "std");
            catalog(Arc::new(consumer.clone()))?
        };
        let correspondence = {
            let _phase = self.profile.span("std.consumer.type_correspondence", "std");
            NativeTypeCorrespondence::build(&source, &target).map_err(|e| e.to_string())?
        };
        let mut callables = std::collections::BTreeSet::new();
        for symbol in consumer.referenced_standard_symbols() {
            if let Some(binding) = consumer.std_registry.body_binding(symbol) {
                callables.insert(binding.callable.clone());
            }
        }
        for identity in consumer.referenced_standard_methods() {
            let callable = etas_code::CallableId::method(identity);
            if source.item_owner(&callable).is_some() {
                callables.insert(callable);
            }
        }
        callables.extend(
            consumer
                .types
                .library_flow_calls
                .values()
                .map(|call| call.callable.clone()),
        );
        let mut interner = TypeInterner::from_store(consumer.type_store.as_ref().clone());
        let source_types = &self.program.type_store;
        let source_lookup = TypeLookupIndex::new(source_types);
        for callable in callables {
            let scope = callable
                .as_method()
                .map(|method| etas_code::descriptor::GenericOwner::Method(method.clone()))
                .or_else(|| {
                    callable.as_declaration().map(|declaration| {
                        etas_code::descriptor::GenericOwner::Declaration(declaration.clone())
                    })
                })
                .ok_or("std callable has no canonical scope")?;
            let implementation_scope = callable.as_method().map(|method| {
                etas_code::descriptor::GenericOwner::Implementation(method.implementation().clone())
            });
            let mut effect_parameters = std::collections::HashMap::new();
            for parameter in source
                .descriptors()
                .data()
                .binders
                .iter()
                .filter(|parameter| {
                    parameter.owner == scope
                        && parameter.kind == etas_code::descriptor::GenericParameterKind::EffectRow
                })
            {
                let target_parameter = target
                    .descriptors()
                    .data()
                    .binders
                    .iter()
                    .find(|candidate| {
                        candidate.owner == scope
                            && candidate.position == parameter.position
                            && candidate.kind == parameter.kind
                    })
                    .ok_or("std ABI has no checked effect parameter correspondence")?;
                effect_parameters.insert(parameter.name.clone(), target_parameter.name.clone());
            }
            let mut declarations = correspondence.declarations().clone();
            let mut targets = std::collections::BTreeMap::new();
            for origin in target.native_type_origins().iter().filter(|origin| {
                origin.scope == scope || Some(&origin.scope) == implementation_scope.as_ref()
            }) {
                let fingerprint = target
                    .descriptors()
                    .type_fingerprint(origin.descriptor)
                    .map_err(|e| e.to_string())?;
                if targets
                    .insert(fingerprint, origin.local)
                    .is_some_and(|previous| previous != origin.local)
                {
                    return Err("std ABI has ambiguous checked type correspondence".into());
                }
            }
            for origin in source.native_type_origins().iter().filter(|origin| {
                origin.scope == scope || Some(&origin.scope) == implementation_scope.as_ref()
            }) {
                let fingerprint = source
                    .descriptors()
                    .type_fingerprint(origin.descriptor)
                    .map_err(|e| e.to_string())?;
                if let Some(target) = targets.get(&fingerprint) {
                    declarations.entry(origin.local).or_insert(*target);
                }
            }
            let item = source
                .item_owner(&callable)
                .ok_or("std callable has no checked body")?;
            let signature = instantiation::signature(&self.program.types, item)
                .ok_or("std body has no checked signature")?;
            if let Some(identity) = callable.as_method()
                && let Some(imported) = consumer
                    .types
                    .external_methods
                    .iter()
                    .find(|method| &method.identity == identity)
                && let Some(authored) = self
                    .program
                    .types
                    .type_impl_methods
                    .values()
                    .find(|method| method.body == item)
            {
                for (source, target) in authored
                    .owner_params
                    .iter()
                    .zip(&imported.owner_params)
                    .chain(
                        signature
                            .generic_params
                            .iter()
                            .zip(&imported.signature.generic_params),
                    )
                {
                    if source.kind == etas_types::CallableGenericParamKind::Type {
                        declarations.insert(source.subject, target.subject);
                    }
                }
            }
            let row = self
                .program
                .checked_standard_callable_row(&callable, item)?;
            let flow = etas_types::FlowType {
                input: signature.params.clone(),
                output: signature.output,
                effects: Some(row),
            };
            let ty = source_lookup
                .get(source_types, &etas_types::Type::Function(flow))
                .ok_or("std body has no checked public function type")?;
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &self.program.type_store,
                target: &mut interner,
                declaration_types: &declarations,
                effect_names: correspondence.effect_names(),
            })
            .with_relocated_effect_parameters(&effect_parameters)
            .substitute(ty)
            .map_err(|e| format!("std public ABI {callable:?}: {e}"))?;
        }
        // Relocation only interns additional types. Preserve the revision's
        // shared store, and its definition indices, when the ABI already exists.
        if interner.store().len() != consumer.type_store.len() {
            consumer.type_store = Arc::new(interner.into_store());
        }
        Ok(())
    }

    /// Prepare the consumer's checked generic applications against the archived
    /// body graph. This materializes types and invocation proofs; it never
    /// lowers source or checks the library bodies again.
    pub(crate) fn for_consumer(
        self: &Arc<Self>,
        consumer: &mut CheckedProgramData,
    ) -> Result<Arc<Self>, String> {
        let _link = self.profile.span("std.consumer.link", "std");
        loop {
            let before = (
                consumer.type_store.len(),
                consumer.native_instantiations.instances().len(),
            );
            let linked = self.link_consumer(consumer)?;
            let after = (
                consumer.type_store.len(),
                consumer.native_instantiations.instances().len(),
            );
            if before == after {
                return Ok(linked);
            }
        }
    }

    fn link_consumer(
        self: &Arc<Self>,
        consumer: &mut CheckedProgramData,
    ) -> Result<Arc<Self>, String> {
        let prepared_owner = instantiation::prepare(
            instantiation::PreparationInput {
                hir: &consumer.hir,
                types: &TypeOutput {
                    store: consumer.type_store.clone(),
                    facts: consumer.types.clone(),
                    diagnostics: vec![],
                },
                effects: &consumer.effects,
                limits: Default::default(),
                control: None,
            },
            consumer.native_instantiations.instances(),
            None,
        )
        .map_err(|error| match error {
            InstantiationError::Invalid(reason) | InstantiationError::Resource { reason, .. } => {
                reason
            }
            InstantiationError::Interrupted(error) => error.reason,
        })?;
        consumer.type_store = prepared_owner.store;
        consumer.native_instantiations = prepared_owner.plan;
        let mut calls = Vec::new();
        for application in prepared_owner.foreign_flows {
            if let Some(selected) = consumer.types.library_flow_calls.get(&application.site) {
                calls.push((
                    application.site,
                    selected.callable.clone(),
                    application.arguments,
                ));
                continue;
            }
            let Some(SymbolDef::ImportAlias { path, .. }) = consumer
                .symbols
                .get(application.symbol)
                .map(|symbol| &symbol.def)
            else {
                continue;
            };
            let Some(binding) = consumer
                .std_registry
                .lookup_qualified(path)
                .and_then(|symbol| consumer.std_registry.body_binding(symbol.id))
            else {
                continue;
            };
            calls.push((
                application.site,
                binding.callable.clone(),
                application.arguments,
            ));
        }
        let method_calls = prepared_owner
            .foreign_methods
            .into_iter()
            .filter(|application| {
                self.descriptors
                    .headers
                    .iter()
                    .flat_map(|headers| &headers.callables)
                    .any(|header| header.id.as_method() == Some(&application.identity))
            })
            .collect::<Vec<_>>();
        let control_types = requests::collect(consumer)?;
        let mut request_outputs = control_types.requests;
        if calls.is_empty() && method_calls.is_empty() && control_types.values.is_empty() {
            return Ok(self.clone());
        }
        calls.sort_by_key(|(site, _, _)| *site);
        let source = Arc::new(catalog(Arc::new(consumer.clone()))?);
        let base_program = effects::import(&source, &self.program)?;
        let target = catalog(base_program.clone())?;
        let mut correspondence =
            NativeTypeCorrespondence::build(&source, &target).map_err(|e| e.to_string())?;
        // Callback actions may carry caller-owned selectors that do not appear
        // in the callback's public escaping row. Import their checked nominal
        // declaration graph before preparing the SDK invocation proofs.
        let limits = etas_code::descriptor::DescriptorLimits::default();
        let control = Continue;
        let budget = etas_types::TypeSubstitutionBudget::new(
            limits.max_nodes,
            limits.max_edges,
            Some(&control),
        );
        let mut roots = Vec::new();
        let mut rows = Vec::new();
        let mut request_types = control_types.values;
        // Existential arguments carry a checked implementation even when the
        // public callable has no generic parameter for its concrete receiver.
        for pack in consumer.types.existential_packs.values() {
            roots.push(pack.source_type);
            roots.push(pack.object_type);
        }
        for application in &method_calls {
            for argument in &application.arguments {
                match argument {
                    NativeGenericArgument::Type(ty) => roots.push(*ty),
                    NativeGenericArgument::EffectRow(row) => rows.push(row),
                }
            }
            roots.extend(application.owner_bindings.iter().map(|(_, ty)| *ty));
        }
        for (site, _, application) in &calls {
            for argument in application {
                match argument {
                    NativeGenericArgument::Type(ty) => roots.push(*ty),
                    NativeGenericArgument::EffectRow(row) => rows.push(row),
                }
            }
            if let Some(summary) = consumer.effects.expr_effects.get(site)
                && let etas_effects::RequestInstantiationSummary::Available { actions, .. } =
                    &summary.handler_requests
            {
                for action in actions {
                    if let Some(signature) = &action.signature {
                        request_types.extend(signature.params.iter().copied());
                        request_types.push(signature.output);
                        request_outputs.push(signature.output);
                    }
                    if let Some(bindings) = &action.type_bindings {
                        request_types.extend(bindings.iter().map(|(_, ty)| *ty));
                    }
                }
            }
        }
        request_types.sort_unstable();
        request_types.dedup();
        request_outputs.sort_unstable();
        request_outputs.dedup();
        roots.extend(&request_types);
        roots.extend(&request_outputs);
        roots.sort_unstable();
        roots.dedup();
        let imported_methods = methods::select(source.checked_program(), &source, &target, &roots)
            .map_err(|error| format!("select consumer methods: {error}"))?;
        let mut additional_specs = Vec::new();
        for method in &imported_methods {
            let declaration = method.declaration;
            if let etas_types::method::MethodEvidence::Implementation { spec, .. } =
                &declaration.evidence
            {
                additional_specs.push(spec.clone());
            }
            roots.push(declaration.receiver_type);
            roots.extend(&declaration.signature.params);
            roots.push(declaration.signature.output);
            rows.extend(declaration.signature.effects.iter());
            for parameter in declaration
                .owner_params
                .iter()
                .chain(&declaration.signature.generic_params)
            {
                roots.push(parameter.subject);
                for bound in &parameter.bounds {
                    roots.extend(&bound.args);
                    additional_specs.push(bound.spec.clone());
                }
            }
        }
        let (reachable, spec_plan) = crate::code::definition::checked_type_and_spec_plan(
            crate::code::definition::CheckedTypeAndSpecPlanInput {
                source: consumer,
                target: &base_program.std_registry,
                target_definitions: Some(&target),
                additional_specs: &additional_specs,
                roots: &roots,
                rows: &rows,
                declarations: correspondence.declarations(),
                identity: &|spec| source.native_spec_identity(spec).cloned(),
                max_depth: limits.max_expansion_depth,
                control: &control,
                budget: &budget,
            },
        )
        .map_err(|e| format!("consumer type/spec closure: {e}"))?;
        let identities = reachable
            .iter()
            .filter_map(|ty| {
                source
                    .native_type_declaration(*ty)
                    .map(|identity| (*ty, identity.clone()))
            })
            .collect();
        let mut interner = TypeInterner::from_store(base_program.type_store.as_ref().clone());
        correspondence
            .reserve_parameters(&source, &reachable, &mut interner, &budget)
            .map_err(|e| relocation_error("consumer binder relocation", &consumer.type_store, e))?;
        let mut declarations = correspondence.declarations().clone();
        for ty in &reachable {
            if declarations.contains_key(ty) {
                continue;
            }
            if let Some(etas_types::Type::AgentContext(context)) = consumer.type_store.get(*ty) {
                let method =
                    match context {
                        etas_types::AgentContextRef::Source(symbol) => {
                            Box::new(source.identity_index().method(*symbol).cloned().ok_or(
                                "std invocation context has no checked agent method identity",
                            )?)
                        }
                        etas_types::AgentContextRef::Portable(method) => method.clone(),
                    };
                let target = interner.intern(etas_types::Type::AgentContext(
                    etas_types::AgentContextRef::Portable(method),
                ));
                declarations.insert(*ty, target);
            }
        }
        methods::reserve_parameters(&imported_methods, &mut interner, &mut declarations)?;
        let foreign = etas_types::ty::relocate_foreign_type_graph(
            interner,
            etas_types::ty::ForeignTypeGraphInput {
                source: &consumer.type_store,
                facts: &consumer.types,
                target_facts: &base_program.types,
                reachable: &reachable,
                identities: &identities,
                declarations: &declarations,
                effect_names: correspondence.effect_names(),
                specs: &spec_plan.mapping,
                max_depth: limits.max_expansion_depth,
                budget: &budget,
            },
        )
        .map_err(|e| relocation_error("consumer into std", &consumer.type_store, e))?;
        let mut interner = foreign.interner;
        let declarations = foreign.declarations;
        let mut foreign_type_bindings = base_program
            .foreign_type_bindings
            .extend_checked(
                &source,
                &identities,
                &declarations,
                &target,
                interner.store(),
                &control,
            )
            .map_err(|e| e.to_string())?;
        foreign_type_bindings
            .extend_specs_checked(&source, &spec_plan.mapping, &control)
            .map_err(|e| e.to_string())?;
        let method_declarations = methods::relocate(
            &imported_methods,
            consumer,
            &base_program,
            &spec_plan.mapping,
            &mut interner,
            &declarations,
            correspondence.effect_names(),
        )
        .map_err(|error| format!("consumer method relocation: {error}"))?;
        foreign_type_bindings
            .extend_methods_checked(&source, &method_declarations, &control)
            .map_err(|e| e.to_string())?;
        let mut seeds = base_program.native_instantiations.instances().to_vec();
        let first_consumer_call = seeds.len();
        let mut control_roots = Vec::new();
        {
            let mut relocation = TypeSubstitutionEngine::relocating_materialized(
                TypeRelocationMaterializationInput {
                    source: &consumer.type_store,
                    target: &mut interner,
                    declaration_types: &declarations,
                    effect_names: correspondence.effect_names(),
                },
            )
            .with_spec_identities(&spec_plan.mapping);

            for ty in request_types {
                control_roots.push(relocation.substitute(ty).map_err(|e| {
                    relocation_error("consumer argument relocation", &consumer.type_store, e)
                })?);
            }
            for output in &mut request_outputs {
                *output = relocation.substitute(*output).map_err(|e| {
                    relocation_error("consumer argument relocation", &consumer.type_store, e)
                })?;
            }
            for (_, callable, application) in calls {
                let owner = target
                    .item_owner(&callable)
                    .ok_or("std generic callable has no checked body")?;
                let arguments = application
                    .into_iter()
                    .map(|argument| {
                        Ok(match argument {
                            NativeGenericArgument::Type(ty) => NativeGenericArgument::Type(
                                relocation.substitute(ty).map_err(|e| {
                                    relocation_error(
                                        "consumer argument relocation",
                                        &consumer.type_store,
                                        e,
                                    )
                                })?,
                            ),
                            NativeGenericArgument::EffectRow(row) => {
                                NativeGenericArgument::EffectRow(
                                    relocation.substitute_effect_row(row).map_err(|e| {
                                        relocation_error(
                                            "consumer argument relocation",
                                            &consumer.type_store,
                                            e,
                                        )
                                    })?,
                                )
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                seeds.push(NativeInstantiation {
                    owner,
                    arguments,
                    owner_bindings: vec![],
                });
            }
            for application in method_calls {
                let item = target
                    .item_owner(&etas_code::CallableId::method(application.identity.clone()))
                    .ok_or("std generic method has no checked body")?;
                let imported = consumer
                    .types
                    .external_methods
                    .iter()
                    .find(|method| method.identity == application.identity)
                    .ok_or("std method has no checked imported owner parameters")?;
                let authored = base_program
                    .types
                    .type_impl_methods
                    .values()
                    .find(|method| method.body == item)
                    .ok_or("std method has no checked source owner parameters")?;
                let arguments = application
                    .arguments
                    .into_iter()
                    .map(|argument| {
                        Ok(match argument {
                            NativeGenericArgument::Type(ty) => NativeGenericArgument::Type(
                                relocation.substitute(ty).map_err(|e| {
                                    relocation_error(
                                        "consumer argument relocation",
                                        &consumer.type_store,
                                        e,
                                    )
                                })?,
                            ),
                            NativeGenericArgument::EffectRow(row) => {
                                NativeGenericArgument::EffectRow(
                                    relocation.substitute_effect_row(row).map_err(|e| {
                                        relocation_error(
                                            "consumer argument relocation",
                                            &consumer.type_store,
                                            e,
                                        )
                                    })?,
                                )
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let owner_bindings = application
                    .owner_bindings
                    .into_iter()
                    .map(|(parameter, argument)| {
                        let position = imported
                            .owner_params
                            .iter()
                            .position(|provided| provided.subject == parameter)
                            .ok_or("std method invocation has no checked owner binder")?;
                        Ok((
                            authored.owner_params[position].subject,
                            relocation.substitute(argument).map_err(|e| {
                                relocation_error(
                                    "consumer argument relocation",
                                    &consumer.type_store,
                                    e,
                                )
                            })?,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                seeds.push(NativeInstantiation {
                    owner: item,
                    arguments,
                    owner_bindings,
                });
            }
        }
        // A source Handler can consume requests performed only by its caller.
        // Prepare its continuation carrier in the SDK's checked type arena;
        // its own HIR has no Perform expression for those caller result types.
        if let Some(owner) = base_program.types.known_std_types.execution.continuation {
            control_roots.extend(
                request_outputs
                    .into_iter()
                    .map(|output| {
                        interner.intern(etas_types::Type::Applied {
                            constructor: etas_types::TypeConstructorId(owner.0),
                            args: vec![output],
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        }
        let spec_facts = spec_plan
            .materialize(crate::code::definition::SpecMaterializationInput {
                source: &consumer.type_store,
                target: &mut interner,
                declarations: &declarations,
                effect_names: correspondence.effect_names(),
                reachable: &reachable,
                max_depth: limits.max_expansion_depth,
                budget: &budget,
            })
            .map_err(|e| e.to_string())?;
        let (facts, reflection_targets) = base_program
            .reflection_targets
            .extend_checked_types(
                &base_program.types,
                interner.store(),
                foreign.type_generics,
                foreign.enum_layouts,
                spec_facts,
                Some(&control),
            )
            .map_err(|error| match error {
                crate::code::reflection::ReflectionTargetError::Invalid(reason) => {
                    reason.to_string()
                }
                crate::code::reflection::ReflectionTargetError::Interrupted(error) => error.reason,
            })?;
        let methods = facts
            .methods
            .with_additional_declarations(interner.store(), method_declarations);
        let (facts, reflection_targets) = reflection_targets.with_method_index(Arc::new(methods));
        etas_types::ty::TypeRepresentationMaterializer::with_acyclic_enum_payloads(
            interner.store(),
            &facts.enum_layouts,
        )
        .and_then(|mut representations| representations.materialize(&mut interner, control_roots))
        .map_err(|error| error.to_string())?;
        let types = TypeOutput {
            store: base_program.type_store.clone(),
            facts: facts.clone(),
            diagnostics: vec![],
        };
        let prepared = instantiation::prepare(
            instantiation::PreparationInput {
                hir: &base_program.hir,
                types: &types,
                effects: &base_program.effects,
                limits,
                control: Some(&control),
            },
            &seeds,
            Some(interner),
        )
        .map_err(|error| match error {
            InstantiationError::Invalid(reason) | InstantiationError::Resource { reason, .. } => {
                reason
            }
            InstantiationError::Interrupted(error) => error.reason,
        })?;
        let mut program = base_program.as_ref().clone();
        program.toolchain_base = consumer.toolchain_base.clone();
        program.types = facts;
        program.reflection_targets = reflection_targets;
        program.foreign_type_bindings = Arc::new(foreign_type_bindings);
        program.type_store = prepared.store;
        program.native_instantiations = prepared.plan;
        owner::prepare(
            consumer,
            &program,
            &prepared.foreign_methods,
            &seeds[first_consumer_call..],
            limits,
            &control,
        )
        .map_err(|error| format!("prepare std method owner: {error}"))?;
        let caller = Arc::new(catalog(Arc::new(consumer.clone()))?);
        let imported = program.types.methods.entries().filter(|method| {
            matches!(&method.target, etas_types::method::MethodTarget::External { identity } if program.foreign_method(identity).is_some())
        }).cloned().collect::<Vec<_>>();
        let mut bindings = program.foreign_type_bindings.as_ref().clone();
        bindings
            .extend_methods_checked(&caller, &imported, &control)
            .map_err(|e| e.to_string())?;
        program.foreign_type_bindings = Arc::new(bindings);
        let mut linked = self.as_ref().clone();
        linked.program = Arc::new(program);
        Ok(Arc::new(linked))
    }
}

struct Continue;

fn relocation_error(
    context: &str,
    store: &etas_types::TypeStore,
    error: etas_types::TypeSubstitutionError,
) -> String {
    if let etas_types::TypeSubstitutionError::MissingTypeIdentity(ty) = &error {
        format!(
            "{context}: {error}; type {:?}; binder {:?}",
            store.get(*ty),
            store.parameter_identity(*ty)
        )
    } else {
        format!("{context}: {error}")
    }
}

impl etas_utils::PipelineExecutionControl for Continue {
    fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
        Ok(())
    }
}

fn catalog(program: crate::CheckedProgram) -> Result<DefinitionCatalog, String> {
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::checked_source_modules(&program)
            .map_err(|e| e.to_string())?,
        standard_package: standard_package_key(&program.std_registry).map_err(|e| e.to_string())?,
        external_definitions: DefinitionOrigins::checked_external_definitions(&program)
            .map_err(|e| e.to_string())?,
    };
    DefinitionCatalog::build_for_execution(program, &origins, Default::default())
        .map_err(|e| e.to_string())
}
