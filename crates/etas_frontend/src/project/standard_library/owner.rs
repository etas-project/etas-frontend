use std::{collections::HashMap, sync::Arc};

use etas_types::{TypeInterner, TypeOutput, TypeSubstitutionEngine};

use super::{
    catalog,
    instantiation::{self, ForeignMethodInstantiation, NativeGenericArgument, NativeInstantiation},
};
use crate::{CheckedProgramData, code::definition::NativeTypeCorrespondence};

pub(super) fn prepare(
    owner: &mut CheckedProgramData,
    library: &CheckedProgramData,
    applications: &[ForeignMethodInstantiation],
    limits: etas_code::descriptor::DescriptorLimits,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<(), String> {
    if applications.is_empty() {
        return Ok(());
    }
    let source = Arc::new(catalog(Arc::new(library.clone()))?);
    let target = catalog(Arc::new(owner.clone()))?;
    let correspondence =
        NativeTypeCorrespondence::build(&source, &target).map_err(|e| e.to_string())?;
    let mut roots = Vec::new();
    let mut rows = Vec::new();
    for application in applications {
        for argument in &application.arguments {
            match argument {
                NativeGenericArgument::Type(ty) => roots.push(*ty),
                NativeGenericArgument::EffectRow(row) => rows.push(row),
            }
        }
        roots.extend(
            application
                .owner_bindings
                .iter()
                .map(|(_, argument)| *argument),
        );
    }
    let budget =
        etas_types::TypeSubstitutionBudget::new(limits.max_nodes, limits.max_edges, Some(control));
    let (reachable, specs) = crate::code::definition::checked_type_and_spec_plan(
        crate::code::definition::CheckedTypeAndSpecPlanInput {
            source: library,
            target: &owner.std_registry,
            target_definitions: Some(&target),
            additional_specs: &[],
            roots: &roots,
            rows: &rows,
            declarations: correspondence.declarations(),
            identity: &|spec| source.native_spec_identity(spec).cloned(),
            max_depth: limits.max_expansion_depth,
            control,
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
        .collect::<HashMap<_, _>>();
    let foreign = etas_types::ty::relocate_foreign_type_graph(
        TypeInterner::from_store(owner.type_store.as_ref().clone()),
        etas_types::ty::ForeignTypeGraphInput {
            source: &library.type_store,
            facts: &library.types,
            target_facts: &owner.types,
            reachable: &reachable,
            identities: &identities,
            declarations: correspondence.declarations(),
            effect_names: correspondence.effect_names(),
            specs: &specs.mapping,
            max_depth: limits.max_expansion_depth,
            budget: &budget,
        },
    )
    .map_err(|e| e.to_string())?;
    let mut interner = foreign.interner;
    let declarations = foreign.declarations;
    let mut bindings = owner
        .foreign_type_bindings
        .extend_checked(
            &source,
            &identities,
            &declarations,
            interner.store(),
            control,
        )
        .map_err(|e| e.to_string())?;
    bindings
        .extend_specs_checked(&source, &specs.mapping, control)
        .map_err(|e| e.to_string())?;
    let mut seeds = owner.native_instantiations.instances().to_vec();
    {
        let mut relocation = TypeSubstitutionEngine::relocating_materialized(
            etas_types::ty::TypeRelocationMaterializationInput {
                source: &library.type_store,
                target: &mut interner,
                declaration_types: &declarations,
                effect_names: correspondence.effect_names(),
            },
        )
        .with_spec_identities(&specs.mapping);
        for application in applications {
            let (origin, item, imported) = library
                .foreign_method(&application.identity)
                .ok_or("SDK invocation has no checked caller method origin")?;
            if !Arc::ptr_eq(origin.hir.shared_program(), owner.hir.shared_program()) {
                return Err("SDK invocation belongs to a different caller revision".into());
            }
            let source_method = owner.types.methods.entries().find(|method| {
                matches!(method.target, etas_types::method::MethodTarget::Source { callable, .. } if callable == item)
            }).ok_or("caller method has no checked signature")?;
            let arguments = application
                .arguments
                .iter()
                .map(|argument| {
                    Ok(match argument {
                        NativeGenericArgument::Type(ty) => NativeGenericArgument::Type(
                            relocation.substitute(*ty).map_err(|e| e.to_string())?,
                        ),
                        NativeGenericArgument::EffectRow(row) => NativeGenericArgument::EffectRow(
                            relocation
                                .substitute_effect_row(row.clone())
                                .map_err(|e| e.to_string())?,
                        ),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let owner_bindings = application
                .owner_bindings
                .iter()
                .map(|(parameter, argument)| {
                    let position = imported
                        .owner_params
                        .iter()
                        .position(|provided| provided.subject == *parameter)
                        .ok_or("SDK method invocation has no checked owner binder")?;
                    Ok((
                        source_method.owner_params[position].subject,
                        relocation
                            .substitute(*argument)
                            .map_err(|e| e.to_string())?,
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
    let spec_facts = specs
        .materialize(crate::code::definition::SpecMaterializationInput {
            source: &library.type_store,
            target: &mut interner,
            declarations: &declarations,
            effect_names: correspondence.effect_names(),
            reachable: &reachable,
            max_depth: limits.max_expansion_depth,
            budget: &budget,
        })
        .map_err(|e| e.to_string())?;
    let (facts, reflection_targets) = owner
        .reflection_targets
        .extend_checked_types(
            &owner.types,
            interner.store(),
            foreign.type_generics,
            foreign.enum_layouts,
            spec_facts,
            Some(control),
        )
        .map_err(|error| match error {
            crate::code::reflection::ReflectionTargetError::Invalid(reason) => reason.to_string(),
            crate::code::reflection::ReflectionTargetError::Interrupted(error) => error.reason,
        })?;
    let prepared = instantiation::prepare(
        instantiation::PreparationInput {
            hir: &owner.hir,
            types: &TypeOutput {
                store: owner.type_store.clone(),
                facts: facts.clone(),
                diagnostics: vec![],
            },
            effects: &owner.effects,
            limits,
            control: Some(control),
        },
        &seeds,
        Some(interner),
    )
    .map_err(|error| match error {
        instantiation::InstantiationError::Invalid(reason)
        | instantiation::InstantiationError::Resource { reason, .. } => reason,
        instantiation::InstantiationError::Interrupted(error) => error.reason,
    })?;
    owner.types = facts;
    owner.reflection_targets = reflection_targets;
    owner.type_store = prepared.store;
    owner.native_instantiations = prepared.plan;
    owner.foreign_type_bindings = Arc::new(bindings);
    Ok(())
}
