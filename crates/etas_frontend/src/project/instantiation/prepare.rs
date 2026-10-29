use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
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
}

pub(crate) struct PreparationInput<'a> {
    pub hir: &'a HirRevision,
    pub types: &'a TypeOutput,
    pub effects: &'a etas_effects::EffectFacts,
    pub limits: etas_code::descriptor::DescriptorLimits,
    pub control: Option<&'a dyn PipelineExecutionControl>,
}

pub(crate) fn prepare(
    input: PreparationInput<'_>,
    seeds: &[NativeInstantiation],
    interner: Option<TypeInterner>,
) -> Result<PreparedInstantiations, InstantiationError> {
    let PreparationInput {
        hir,
        types,
        effects,
        limits,
        control,
    } = input;
    if types.facts.generic_instantiations.is_empty() && seeds.is_empty() {
        return Ok(PreparedInstantiations {
            store: types.store.clone(),
            plan: Arc::default(),
        });
    }
    let index = hir.index();
    let mut calls = BTreeMap::<HirItemId, Vec<HirExprId>>::new();
    for call in types.facts.generic_instantiations.keys() {
        if !effects
            .native_call_targets
            .get(call)
            .is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| matches!(target, etas_effects::EffectUnit::Item(_)))
            })
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
    }
    if calls.is_empty() && seeds.is_empty() {
        return Ok(PreparedInstantiations {
            store: types.store.clone(),
            plan: Arc::default(),
        });
    }
    let mut roots = BTreeMap::<HirItemId, Vec<TypeId>>::new();
    for (expr, ty) in &types.facts.expr_types {
        let owner = index
            .expr_item
            .get(expr)
            .ok_or("checked expression type has no body owner".to_owned())?;
        roots.entry(*owner).or_default().push(*ty);
    }
    for (expr, method) in &types.facts.std_method_calls {
        let owner = index
            .expr_item
            .get(expr)
            .ok_or("checked intrinsic has no body owner".to_owned())?;
        roots.entry(*owner).or_default().push(method.signature);
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
        })
        .map(|owner| NativeInstantiation {
            owner: *owner,
            arguments: vec![],
        })
        .collect::<VecDeque<_>>();
    pending.extend(seeds.iter().cloned());
    let mut seen = HashSet::new();
    let mut plan = NativeInstantiationPlan::default();
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
            let mut substitution =
                TypeSubstitutionEngine::materializing(&mut interner, &type_bindings, None)
                    .with_effect_arguments(&row_bindings)
                    .with_expansion_depth(limits.max_expansion_depth)
                    .with_execution_control(control);
            if let Some(signature) = template {
                for ty in signature.params.iter().copied().chain([signature.output]) {
                    materialized.push(
                        substitution
                            .substitute(ty)
                            .map_err(InstantiationError::from)?,
                    );
                }
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
                let fact = &types.facts.generic_instantiations[call];
                for target in effects.native_call_targets.get(call).into_iter().flatten() {
                    let etas_effects::EffectUnit::Item(item) = target else {
                        continue;
                    };
                    let target_signature = signature(&types.facts, *item)
                        .ok_or("native generic target has no checked signature".to_owned())?;
                    let method_fact = match hir.exprs.get(*call) {
                        Some(etas_hir::HirExpr::SpecMethodCall {
                            spec_path, method, ..
                        }) => {
                            let etas_hir::ResolveResult::Resolved(symbol) = spec_path.resolution
                            else {
                                return Err("spec invocation has no resolved spec".into());
                            };
                            let symbol = hir
                                .canonical_source_symbol(symbol)
                                .ok_or("spec invocation has no canonical spec")?;
                            let method = types
                                .facts
                                .spec_signatures
                                .get(&symbol)
                                .and_then(|spec| {
                                    spec.methods
                                        .iter()
                                        .find(|candidate| candidate.name == *method)
                                })
                                .ok_or("spec invocation has no checked method declaration")?;
                            let declaration = method
                                .signature_in(&types.facts)
                                .ok_or("spec invocation has no checked method signature")?;
                            Some(method.implementation_instantiation(
                                declaration,
                                target_signature,
                                fact,
                            )?)
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
                    });
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
            if !instance.arguments.is_empty() {
                plan.instances.push(instance);
            }
            drop(substitution);
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
    })
}

fn bind_call(
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
