use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use etas_code::descriptor::DescriptorLimits;
use etas_effects::EffectUnit;
use etas_types::{
    TypeInterner, TypeOutput, TypeSubstitutionEngine, ty::TypeRelocationMaterializationInput,
};
use etas_utils::PipelineExecutionControl;

use crate::{
    CheckedProgram,
    code::definition::{DefinitionCatalog, FrozenDefinitionEnvironment, NativeTypeCorrespondence},
    project::instantiation::{
        self, InstantiationError, NativeGenericArgument, NativeInstantiation,
    },
};

/// Prepare frozen source applications referenced by the candidate, without
/// changing the source program or importing candidate body/effect facts.
pub(super) fn prepare(
    frozen: &FrozenDefinitionEnvironment,
    candidate: &CheckedProgram,
    limits: DescriptorLimits,
    control: Option<&dyn PipelineExecutionControl>,
) -> Result<CheckedProgram, InstantiationError> {
    let original = frozen.checked();
    let mut owners = BTreeSet::new();
    let mut interfaces = BTreeSet::new();
    let mut callable_value_reflection = false;
    for (expr, fact) in candidate.checked_reflections() {
        let value = matches!(
            fact.target,
            etas_types::CheckedReflectionTarget::CallableValue { .. }
        );
        let declaration = candidate.types.generic_instantiations.contains_key(expr)
            && matches!(
                fact.target,
                etas_types::CheckedReflectionTarget::Callable { .. }
                    | etas_types::CheckedReflectionTarget::ExternalCallable { .. }
            );
        if !value && !declaration {
            continue;
        }
        callable_value_reflection |= value;
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(InstantiationError::Interrupted)?;
        }
        let mut has_native_target = false;
        for target in candidate
            .effects
            .native_call_targets
            .get(expr)
            .into_iter()
            .flatten()
        {
            if let EffectUnit::Item(item) = target {
                owners.insert(*item);
                has_native_target = true;
            }
        }
        if (has_native_target || value)
            && let Some(etas_types::Type::Applied { constructor, args }) =
                candidate.type_store.get(fact.descriptor_type)
            && Some(etas_types::TypeId(constructor.0))
                == candidate.types.known_std_types.code.callable_definition
        {
            let [interface] = args.as_slice() else {
                return Err(InstantiationError::Invalid(
                    "checked reflected callable has inconsistent interface arity".into(),
                ));
            };
            if !matches!(
                candidate.type_store.get(*interface),
                Some(etas_types::Type::Function(_))
            ) {
                return Err(InstantiationError::Invalid(
                    "checked reflected callable interface is not a function type".into(),
                ));
            }
            let owner = *candidate
                .hir
                .index()
                .expr_item
                .get(expr)
                .ok_or("checked reflection has no lexical owner")?;
            if interfaces.len() >= limits.max_nodes && !interfaces.contains(&(owner, *interface)) {
                return Err(InstantiationError::Interrupted(
                    etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                        reason: "frozen reflection interface limit exceeded".into(),
                    },
                ));
            }
            interfaces.insert((owner, *interface));
        }
    }
    if callable_value_reflection {
        // The runtime value may come from any checked native target in the
        // reachable candidate. Preparing its type application does not run it.
        for targets in candidate.effects.native_call_targets.values() {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(InstantiationError::Interrupted)?;
            }
            for target in targets {
                if let EffectUnit::Item(item) = target
                    && matches!(candidate.hir.items.get(*item), Some(etas_hir::HirItem::Flow(flow))
                        if !candidate.types.agent_methods.contains_key(&flow.symbol))
                {
                    owners.insert(*item);
                }
            }
        }
    }
    if owners.is_empty() {
        return Ok(original.clone());
    }
    let source_catalog = DefinitionCatalog::build(
        original.clone(),
        &frozen
            .origins_for(original)
            .map_err(|e| InstantiationError::Invalid(e.to_string()))?,
        limits,
    )
    .map_err(super::projection_failure)?;
    let target_catalog = DefinitionCatalog::build(
        candidate.clone(),
        &frozen
            .origins_for(candidate)
            .map_err(|e| InstantiationError::Invalid(e.to_string()))?,
        limits,
    )
    .map_err(super::projection_failure)?;
    let correspondence = NativeTypeCorrespondence::build(&target_catalog, &source_catalog)
        .map_err(super::projection_failure)?;
    let mut interner = TypeInterner::from_store((*original.type_store).clone());
    let budget =
        etas_types::TypeSubstitutionBudget::new(limits.max_nodes, limits.max_edges, control);
    let mut seeds = original.native_instantiations.instances().to_vec();
    let mut candidate_types = None;
    let mut resolved_interfaces = BTreeSet::new();
    for (owner, interface) in interfaces {
        let signature = instantiation::signature(&candidate.types, owner)
            .ok_or("checked reflection owner has no callable signature")?;
        if signature.generic_params.is_empty() {
            resolved_interfaces.insert(interface);
            continue;
        }
        for instance in candidate
            .native_instantiations
            .instances()
            .iter()
            .filter(|value| value.owner == owner)
        {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(InstantiationError::Interrupted)?;
            }
            let (types, rows) = instance
                .bindings(Some(signature))
                .map_err(InstantiationError::Invalid)?;
            let mut source_parameters = HashMap::new();
            for parameter in &signature.generic_params {
                if let Some(ty) = types.get(&parameter.name) {
                    let Some(etas_types::Type::Named(name)) =
                        candidate.type_store.get(parameter.subject)
                    else {
                        return Err(InstantiationError::Invalid(
                            "reflection owner has no checked type parameter identity".into(),
                        ));
                    };
                    let symbol = name
                        .source_parameter
                        .ok_or("reflection owner has no checked type parameter symbol")?;
                    if source_parameters.insert(symbol, *ty).is_some() {
                        return Err(InstantiationError::Invalid(
                            "reflection owner has duplicate type parameter identity".into(),
                        ));
                    }
                }
            }
            let mut substitution = TypeSubstitutionEngine::existing_indexed(
                &candidate.type_store,
                candidate_types.get_or_insert_with(|| {
                    etas_types::ty::TypeLookupIndex::new(&candidate.type_store)
                }),
                &types,
            )
            .with_source_parameters(&source_parameters)
            .with_effect_arguments(&rows)
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
            resolved_interfaces.insert(
                substitution
                    .substitute(interface)
                    .map_err(InstantiationError::from)?,
            );
        }
    }
    {
        let mut relocation =
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &candidate.type_store,
                target: &mut interner,
                declaration_types: correspondence.declarations(),
                effect_names: correspondence.effect_names(),
            })
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
        for interface in resolved_interfaces {
            relocation
                .substitute(interface)
                .map_err(InstantiationError::from)?;
        }
    }
    for instance in candidate.native_instantiations.instances() {
        if !owners.contains(&instance.owner) {
            continue;
        }
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(InstantiationError::Interrupted)?;
        }
        let identity = target_catalog
            .ordinary_flow_identity(instance.owner)
            .ok_or("candidate reflection target has no stable flow identity")?;
        let owner = source_catalog
            .item_owner(identity)
            .ok_or("candidate reflection target has no frozen source flow")?;
        let source_signature = instantiation::signature(&original.types, owner)
            .ok_or("frozen reflection target has no checked signature")?;
        let target_signature = instantiation::signature(&candidate.types, instance.owner)
            .ok_or("candidate reflection target has no checked signature")?;
        if source_signature.generic_params.len() != target_signature.generic_params.len()
            || source_signature
                .generic_params
                .iter()
                .zip(&target_signature.generic_params)
                .any(|(a, b)| a.name != b.name || a.kind != b.kind)
        {
            return Err(InstantiationError::Invalid(
                "frozen reflection target changed generic binders".into(),
            ));
        }
        let mut relocation =
            TypeSubstitutionEngine::relocating_materialized(TypeRelocationMaterializationInput {
                source: &candidate.type_store,
                target: &mut interner,
                declaration_types: correspondence.declarations(),
                effect_names: correspondence.effect_names(),
            })
            .with_expansion_depth(limits.max_expansion_depth)
            .with_budget(&budget);
        let arguments = instance
            .arguments
            .iter()
            .map(|arg| match arg {
                NativeGenericArgument::Type(ty) => {
                    relocation.substitute(*ty).map(NativeGenericArgument::Type)
                }
                NativeGenericArgument::EffectRow(row) => relocation
                    .substitute_effect_row(row.clone())
                    .map(NativeGenericArgument::EffectRow),
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(InstantiationError::from)?;
        seeds.push(NativeInstantiation { owner, arguments });
    }
    let types = TypeOutput {
        facts: original.types.clone(),
        store: original.type_store.clone(),
        diagnostics: Vec::new(),
    };
    let prepared = instantiation::prepare(
        instantiation::PreparationInput {
            hir: &original.hir,
            types: &types,
            effects: &original.effects,
            limits,
            control,
        },
        &seeds,
        Some(interner),
    )?;
    let mut program = (**original).clone();
    program.type_store = prepared.store;
    program.native_instantiations = prepared.plan;
    Ok(Arc::new(program))
}
