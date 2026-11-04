use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::Arc,
};

use etas_hir::{HirExprId, HirItemId, HirRevision};
use etas_types::{
    CallableGenericParamKind, GenericInstantiationFact, TypeId, TypeInterner, TypeOutput,
    TypeStore, TypeSubstitutionEngine,
};
use etas_utils::{PipelineExecutionControl, PipelineInterruption, PipelineInterruptionKind};

use super::{NativeGenericArgument, NativeInstantiation, NativeInstantiationPlan, signature};

pub(crate) enum InstantiationError {
    Invalid(String),
    Interrupted(PipelineInterruption),
    Resource { owner: HirItemId, reason: String },
}
impl From<etas_types::TypeSubstitutionError> for InstantiationError {
    fn from(error: etas_types::TypeSubstitutionError) -> Self {
        match error {
            etas_types::TypeSubstitutionError::Interrupted(interruption) => {
                Self::Interrupted(interruption)
            }
            etas_types::TypeSubstitutionError::ExpansionDepth(_)
            | etas_types::TypeSubstitutionError::WorkLimit { .. }
            | etas_types::TypeSubstitutionError::RepresentationLimit(_) => {
                Self::Interrupted(PipelineInterruption {
                    kind: PipelineInterruptionKind::ResourceLimit,
                    reason: error.to_string(),
                })
            }
            _ => Self::Invalid(error.to_string()),
        }
    }
}
impl From<String> for InstantiationError {
    fn from(reason: String) -> Self {
        Self::Invalid(reason)
    }
}
impl From<&str> for InstantiationError {
    fn from(reason: &str) -> Self {
        Self::Invalid(reason.into())
    }
}

pub(crate) struct PreparedInstantiations {
    pub store: Arc<TypeStore>,
    pub plan: Arc<NativeInstantiationPlan>,
    pub method_bodies: BTreeSet<HirItemId>,
}

pub(crate) struct PreparationInput<'a> {
    pub hir: &'a HirRevision,
    pub types: &'a TypeOutput,
    pub effects: &'a etas_effects::EffectFacts,
    pub limits: etas_code::descriptor::DescriptorLimits,
    pub control: Option<&'a dyn PipelineExecutionControl>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparationPhase {
    Types,
    Effects,
}

pub(crate) struct InvocationInput<'a> {
    pub hir: &'a HirRevision,
    pub types: &'a TypeOutput,
    pub targets: &'a HashMap<HirExprId, BTreeSet<HirItemId>>,
    pub limits: etas_code::descriptor::DescriptorLimits,
    pub control: Option<&'a dyn PipelineExecutionControl>,
    pub phase: PreparationPhase,
}

pub(crate) fn prepare(
    input: PreparationInput<'_>,
    seeds: &[NativeInstantiation],
    interner: Option<TypeInterner>,
) -> Result<PreparedInstantiations, InstantiationError> {
    let targets = input
        .effects
        .native_call_targets
        .iter()
        .filter_map(|(site, targets)| {
            let items = targets
                .iter()
                .filter_map(|target| match target {
                    etas_effects::EffectUnit::Item(item) => Some(*item),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            (!items.is_empty()).then_some((*site, items))
        })
        .collect();
    prepare_from_targets(
        InvocationInput {
            hir: input.hir,
            types: input.types,
            targets: &targets,
            limits: input.limits,
            control: input.control,
            phase: PreparationPhase::Effects,
        },
        seeds,
        interner,
    )
}

pub(crate) fn prepare_from_targets(
    input: InvocationInput<'_>,
    seeds: &[NativeInstantiation],
    interner: Option<TypeInterner>,
) -> Result<PreparedInstantiations, InstantiationError> {
    let InvocationInput {
        hir,
        types,
        targets,
        limits,
        control,
        phase,
    } = input;
    if types.facts.generic_instantiations.is_empty()
        && types.facts.checked_method_calls.is_empty()
        && types.facts.existential_packs.is_empty()
        && seeds.is_empty()
    {
        return Ok(PreparedInstantiations {
            store: types.store.clone(),
            plan: Arc::default(),
            method_bodies: BTreeSet::new(),
        });
    }
    let index = hir.index();
    let mut calls = BTreeMap::<HirItemId, Vec<HirExprId>>::new();
    for call in types
        .facts
        .generic_instantiations
        .keys()
        .chain(types.facts.checked_method_calls.keys())
        .chain(types.facts.existential_packs.keys())
    {
        if hir.program().is_expr_optimized_out(*call) {
            continue;
        }
        if targets.get(call).is_none_or(BTreeSet::is_empty)
            && !types.facts.checked_method_calls.contains_key(call)
            && !types.facts.existential_packs.contains_key(call)
            && super::targets::external_signature(hir, types, *call).is_none()
        {
            continue;
        }
        let owner = index
            .expr_item
            .get(call)
            .ok_or("generic invocation has no checked owner".to_owned())?;
        calls.entry(*owner).or_default().push(*call);
    }
    for sites in calls.values_mut() {
        sites.sort();
        sites.dedup();
    }
    if calls.is_empty() && seeds.is_empty() {
        return Ok(PreparedInstantiations {
            store: types.store.clone(),
            plan: Arc::default(),
            method_bodies: BTreeSet::new(),
        });
    }
    let mut roots = BTreeMap::<HirItemId, Vec<TypeId>>::new();
    for (expr, ty) in &types.facts.expr_types {
        if hir.program().is_expr_optimized_out(*expr) {
            continue;
        }
        let owner = index
            .expr_item
            .get(expr)
            .ok_or("checked expression type has no body owner".to_owned())?;
        roots.entry(*owner).or_default().push(*ty);
    }
    for (expr, method) in &types.facts.std_intrinsic_calls {
        if hir.program().is_expr_optimized_out(*expr) {
            continue;
        }
        let owner = index
            .expr_item
            .get(expr)
            .ok_or("checked intrinsic has no body owner".to_owned())?;
        roots.entry(*owner).or_default().push(method.callable_type);
    }
    for roots in roots.values_mut() {
        roots.sort();
        roots.dedup();
    }
    let mut pending = calls
        .keys()
        .filter(|owner| {
            signature(&types.facts, **owner)
                .is_none_or(|signature| signature.generic_params.is_empty())
                && super::owner_parameters(hir, &types.facts, **owner).is_empty()
        })
        .map(|owner| NativeInstantiation {
            owner: *owner,
            arguments: vec![],
            owner_bindings: vec![],
        })
        .collect::<VecDeque<_>>();
    pending.extend(seeds.iter().cloned());
    let mut seen = HashSet::new();
    let mut plan = NativeInstantiationPlan::default();
    let witnesses = super::witness::WitnessResolver::new(types);
    let mut method_bodies = BTreeSet::new();
    let mut interner = interner.unwrap_or_else(|| TypeInterner::from_store((*types.store).clone()));
    let mut materialized = Vec::new();
    let mut representations =
        etas_types::ty::TypeRepresentationMaterializer::with_acyclic_enum_payloads(
            interner.store(),
            &types.facts.enum_layouts,
        )?;
    while let Some(instance) = pending.pop_front() {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(InstantiationError::Interrupted)?;
        }
        if !seen.insert(instance.clone()) {
            continue;
        }
        if seen.len() > limits.max_nodes {
            return Err(InstantiationError::Resource {
                owner: instance.owner,
                reason: format!(
                    "native invocation graph exceeds {} instances",
                    limits.max_nodes
                ),
            });
        }
        let owner = instance.owner;
        (|| -> Result<(), InstantiationError> {
            let template = signature(&types.facts, instance.owner);
            let (type_bindings, row_bindings) = instance.bindings(template)?;
            let parameters = instance.type_parameters(template);
            let mut substitution =
                TypeSubstitutionEngine::materializing(&mut interner, &type_bindings, None)
                    .with_type_parameters(&parameters)
                    .with_effect_arguments(&row_bindings)
                    .with_expansion_depth(limits.max_expansion_depth)
                    .with_execution_control(control);
            let mut callable_type = None;
            if let Some(signature) = template {
                let params = signature
                    .params
                    .iter()
                    .map(|ty| substitution.substitute(*ty))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(InstantiationError::from)?;
                let output = substitution
                    .substitute(signature.output)
                    .map_err(InstantiationError::from)?;
                let effects = signature
                    .effects
                    .clone()
                    .map(|row| substitution.substitute_effect_row(row))
                    .transpose()
                    .map_err(InstantiationError::from)?;
                materialized.extend(params.iter().copied());
                materialized.push(output);
                callable_type = Some(etas_types::FlowType {
                    input: params,
                    output,
                    effects,
                });
                for row in signature.effects.iter().chain(&signature.requested_actions) {
                    let row = substitution
                        .substitute_effect_row(row.clone())
                        .map_err(InstantiationError::from)?;
                    materialized.extend(
                        row.effects
                            .into_iter()
                            .flat_map(|effect| effect.args)
                            .filter_map(|arg| match arg {
                                etas_types::EffectArgRef::Type(ty) => Some(ty),
                                _ => None,
                            }),
                    );
                }
            }
            for ty in roots.get(&instance.owner).into_iter().flatten() {
                materialized.push(
                    substitution
                        .substitute(*ty)
                        .map_err(InstantiationError::from)?,
                );
            }
            for call in calls.get(&instance.owner).into_iter().flatten() {
                if let Some(signature) = super::targets::external_signature(hir, types, *call)
                    && let Some(fact) = types.facts.generic_instantiations.get(call)
                {
                    let dictionary = witnesses.bound_bodies(signature, fact, &mut substitution)?;
                    method_bodies.extend(dictionary.bodies);
                    pending.extend(dictionary.applications);
                }
                if let Some(method) = types.facts.checked_method_calls.get(call)
                    && matches!(
                        method.target,
                        etas_types::method::MethodTarget::BoundSpec { .. }
                    )
                {
                    if let Some(application) =
                        witnesses.method_application(method, &mut substitution)?
                    {
                        method_bodies.insert(application.body);
                        match application.instance {
                            Some(instance) => pending.push_back(instance),
                            None if phase == PreparationPhase::Types => {}
                            None => {
                                return Err(
                                    "native invocation has unmaterialized deferred effect rows"
                                        .into(),
                                );
                            }
                        }
                        continue;
                    }
                }
                if let Some(pack) = types.facts.existential_packs.get(call) {
                    let pack = witnesses.pack_bodies(pack, &mut substitution)?;
                    method_bodies.extend(pack.bodies);
                    pending.extend(pack.applications);
                }
                let fact = match types.facts.checked_method_calls.get(call) {
                    Some(method) => &method.instantiation,
                    None => match types.facts.generic_instantiations.get(call) {
                        Some(fact) => fact,
                        None => continue,
                    },
                };
                if !fact.deferred_effect_row_obligations.is_empty()
                    && phase == PreparationPhase::Types
                {
                    continue;
                }
                for item in targets.get(call).into_iter().flatten() {
                    let target_signature = signature(&types.facts, *item)
                        .ok_or("native generic target has no checked signature".to_owned())?;
                    let method_fact = match types
                        .facts
                        .checked_method_calls
                        .get(call)
                        .map(|fact| &fact.target)
                    {
                        Some(etas_types::method::MethodTarget::BoundSpec { member, .. }) => {
                            Some(witnesses.instantiation(member, target_signature, fact)?)
                        }
                        _ => None,
                    };
                    let arguments = bind_call(
                        &mut substitution,
                        target_signature,
                        method_fact.as_ref().unwrap_or(fact),
                    )?;
                    pending.push_back(NativeInstantiation {
                        owner: *item,
                        arguments,
                        owner_bindings: types
                            .facts
                            .checked_method_calls
                            .get(call)
                            .into_iter()
                            .flat_map(|fact| &fact.owner_bindings)
                            .map(|(parameter, argument)| {
                                Ok((*parameter, substitution.substitute(*argument)?))
                            })
                            .collect::<Result<_, etas_types::TypeSubstitutionError>>()?,
                    });
                    if types.facts.checked_method_calls.contains_key(call) {
                        method_bodies.insert(*item);
                    }
                }
            }
            for argument in &instance.arguments {
                match argument {
                    NativeGenericArgument::Type(ty) => materialized.push(*ty),
                    NativeGenericArgument::EffectRow(row) => materialized.extend(
                        row.effects
                            .iter()
                            .flat_map(|effect| &effect.args)
                            .filter_map(|arg| match arg {
                                etas_types::EffectArgRef::Type(ty) => Some(*ty),
                                _ => None,
                            }),
                    ),
                }
            }
            if !instance.arguments.is_empty() || !instance.owner_bindings.is_empty() {
                plan.push(instance);
            }
            drop(substitution);
            if let Some(callable) = callable_type {
                materialized.push(interner.intern(etas_types::Type::Function(callable)));
            }
            let names = HashMap::new();
            let mut validation = TypeSubstitutionEngine::existing(interner.store(), &names)
                .with_expansion_depth(limits.max_expansion_depth)
                .with_execution_control(control);
            for ty in &materialized {
                validation
                    .substitute(*ty)
                    .map_err(InstantiationError::from)?;
            }
            representations
                .materialize(&mut interner, std::mem::take(&mut materialized))
                .map_err(InstantiationError::from)?;
            Ok(())
        })()
        .map_err(|error| match error {
            InstantiationError::Interrupted(interruption)
                if interruption.kind == PipelineInterruptionKind::ResourceLimit =>
            {
                InstantiationError::Resource {
                    owner,
                    reason: interruption.reason,
                }
            }
            other => other,
        })?;
    }
    Ok(PreparedInstantiations {
        store: Arc::new(interner.into_store()),
        plan: Arc::new(plan),
        method_bodies,
    })
}

pub(super) fn bind_call(
    substitution: &mut TypeSubstitutionEngine<'_>,
    target: &etas_types::CallableSignature,
    fact: &GenericInstantiationFact,
) -> Result<Vec<NativeGenericArgument>, InstantiationError> {
    if !fact.deferred_effect_row_obligations.is_empty() {
        return Err("native invocation has unmaterialized deferred effect rows".into());
    }
    let types = fact
        .type_bindings
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let rows = fact
        .effect_row_bindings
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    if target.generic_params.len() != types.len() + rows.len() {
        return Err("native invocation has an inconsistent generic argument count".into());
    }
    target
        .generic_params
        .iter()
        .map(|param| match param.kind {
            CallableGenericParamKind::Type => {
                let ty = types
                    .get(&param.name)
                    .ok_or("native invocation has no checked type binding")?;
                substitution
                    .substitute(*ty)
                    .map(NativeGenericArgument::Type)
                    .map_err(InstantiationError::from)
            }
            CallableGenericParamKind::Effect => {
                let row = rows
                    .get(&param.name)
                    .ok_or("native invocation has no solved effect-row binding")?;
                substitution
                    .substitute_effect_row(row.clone())
                    .map(NativeGenericArgument::EffectRow)
                    .map_err(InstantiationError::from)
            }
        })
        .collect()
}
