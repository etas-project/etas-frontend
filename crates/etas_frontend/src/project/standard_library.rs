use std::sync::Arc;

mod methods;
mod owner;

use etas_hir::SymbolDef;
use etas_types::{
    TypeInterner, TypeOutput, TypeSubstitutionEngine, ty::TypeRelocationMaterializationInput,
};

use super::instantiation::{self, InstantiationError, NativeGenericArgument, NativeInstantiation};
use crate::{
    CheckedProgramData, StandardLibrary,
    code::{
        definition::{DefinitionCatalog, NativeTypeCorrespondence},
        identity::{DefinitionOrigins, standard_package_key},
    },
};

impl StandardLibrary {
    /// Function values cross the SDK boundary with the solved public effect
    /// row. Keep this ABI alongside the authored signature before execution.
    pub(crate) fn materialize_public_abis(
        &self,
        consumer: &mut CheckedProgramData,
    ) -> Result<(), String> {
        let source = catalog(self.program.clone())?;
        let target = catalog(Arc::new(consumer.clone()))?;
        let correspondence =
            NativeTypeCorrespondence::build(&source, &target).map_err(|e| e.to_string())?;
        let mut callables = std::collections::BTreeSet::new();
        for symbol in consumer.symbols.iter() {
            if let SymbolDef::ImportAlias { path, .. } = &symbol.def
                && let Some(binding) = consumer
                    .std_registry
                    .lookup_qualified(path)
                    .and_then(|declaration| consumer.std_registry.body_binding(declaration.id))
            {
                callables.insert(binding.callable.clone());
            }
        }
        for method in consumer.types.methods.entries() {
            if let etas_types::method::MethodTarget::External { identity } = &method.target {
                let callable = etas_code::CallableId::method(identity.clone());
                if source.item_owner(&callable).is_some() {
                    callables.insert(callable);
                }
            }
        }
        let mut interner = TypeInterner::from_store(consumer.type_store.as_ref().clone());
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
            for origin in target
                .native_type_origins()
                .iter()
                .filter(|origin| origin.scope == scope)
            {
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
            for origin in source
                .native_type_origins()
                .iter()
                .filter(|origin| origin.scope == scope)
            {
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
            let contract = self
                .program
                .effects
                .public_contracts
                .iter()
                .find(|contract| contract.item == item)
                .ok_or("std body has no checked public effect contract")?;
            let row = instantiation::checked_row(
                &contract.public_row,
                &self.program.effect_registry,
                Some(signature),
                std::iter::empty(),
            )?;
            let flow = etas_types::FlowType {
                input: signature.params.clone(),
                output: signature.output,
                effects: Some(row),
            };
            let ty = self
                .program
                .type_store
                .iter()
                .find_map(|(id, ty)| {
                    matches!(ty, etas_types::Type::Function(candidate) if candidate == &flow)
                        .then_some(id)
                })
                .ok_or("std body has no checked public function type")?;
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &self.program.type_store,
                target: &mut interner,
                declaration_types: &declarations,
                effect_names: correspondence.effect_names(),
            })
            .with_relocated_effect_parameters(&effect_parameters)
            .substitute(ty)
            .map_err(|e| e.to_string())?;
        }
        consumer.type_store = Arc::new(interner.into_store());
        Ok(())
    }

    /// Prepare the consumer's checked generic applications against the archived
    /// body graph. This materializes types and invocation proofs; it never
    /// lowers source or checks the library bodies again.
    pub(crate) fn for_consumer(
        self: &Arc<Self>,
        consumer: &mut CheckedProgramData,
    ) -> Result<Arc<Self>, String> {
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
        if calls.is_empty() {
            return Ok(self.clone());
        }
        calls.sort_by_key(|(site, _, _)| *site);
        let source = Arc::new(catalog(Arc::new(consumer.clone()))?);
        let target = catalog(self.program.clone())?;
        let correspondence =
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
        let mut request_types = Vec::new();
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
                    }
                    if let Some(bindings) = &action.type_bindings {
                        request_types.extend(bindings.iter().map(|(_, ty)| *ty));
                    }
                }
            }
        }
        request_types.sort_unstable();
        request_types.dedup();
        roots.extend(&request_types);
        let imported_methods = methods::select(source.checked_program(), &source, &target, &roots)?;
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
                target: &self.program.std_registry,
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
        .map_err(|e| e.to_string())?;
        let identities = reachable
            .iter()
            .filter_map(|ty| {
                source
                    .native_type_declaration(*ty)
                    .map(|identity| (*ty, identity.clone()))
            })
            .collect();
        let mut interner = TypeInterner::from_store(self.program.type_store.as_ref().clone());
        let mut declarations = correspondence.declarations().clone();
        methods::reserve_parameters(&imported_methods, &mut interner, &mut declarations)?;
        let foreign = etas_types::ty::relocate_foreign_type_graph(
            interner,
            etas_types::ty::ForeignTypeGraphInput {
                source: &consumer.type_store,
                facts: &consumer.types,
                target_facts: &self.program.types,
                reachable: &reachable,
                identities: &identities,
                declarations: &declarations,
                effect_names: correspondence.effect_names(),
                specs: &spec_plan.mapping,
                max_depth: limits.max_expansion_depth,
                budget: &budget,
            },
        )
        .map_err(|e| e.to_string())?;
        let mut interner = foreign.interner;
        let declarations = foreign.declarations;
        let mut foreign_type_bindings = self
            .program
            .foreign_type_bindings
            .extend_checked(
                &source,
                &identities,
                &declarations,
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
            &self.program,
            &spec_plan.mapping,
            &mut interner,
            &declarations,
            correspondence.effect_names(),
        )?;
        foreign_type_bindings
            .extend_methods_checked(&source, &method_declarations, &control)
            .map_err(|e| e.to_string())?;
        let mut seeds = self.program.native_instantiations.instances().to_vec();
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
                relocation.substitute(ty).map_err(|e| e.to_string())?;
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
                                relocation.substitute(ty).map_err(|e| e.to_string())?,
                            ),
                            NativeGenericArgument::EffectRow(row) => {
                                NativeGenericArgument::EffectRow(
                                    relocation
                                        .substitute_effect_row(row)
                                        .map_err(|e| e.to_string())?,
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
        let (facts, reflection_targets) = self
            .program
            .reflection_targets
            .extend_checked_types(
                &self.program.types,
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
        let types = TypeOutput {
            store: self.program.type_store.clone(),
            facts: facts.clone(),
            diagnostics: vec![],
        };
        let prepared = instantiation::prepare(
            instantiation::PreparationInput {
                hir: &self.program.hir,
                types: &types,
                effects: &self.program.effects,
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
        let mut program = self.program.as_ref().clone();
        program.types = facts;
        program.reflection_targets = reflection_targets;
        program.foreign_type_bindings = Arc::new(foreign_type_bindings);
        program.type_store = prepared.store;
        program.native_instantiations = prepared.plan;
        owner::prepare(
            consumer,
            &program,
            &prepared.foreign_methods,
            limits,
            &control,
        )?;
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
    DefinitionCatalog::build(program, &origins, Default::default()).map_err(|e| e.to_string())
}
