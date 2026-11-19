//! Checked type applications are distinct proof nodes. An unresolved generic
//! witness stays unknown; a concrete caller uses its actual implementation.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_types::{
    CallableGenericParam, CallableGenericParamKind, CallableSignature, GenericInstantiationFact,
    ItemSignature, TypeId, TypeSubstitutionEngine,
    facts::CheckedMethodCallFact,
    method::{MethodEvidence, MethodTarget},
};

use super::{CallContext, EffectUnit, HirExprId};
use crate::facts::{
    CallableEnvironmentId, CallableGenericParameter, CallableGenericValue, CallableValueOrigin,
};

pub(super) type ValueBindings = BTreeMap<crate::ParameterCallTarget, BTreeSet<CallableValueOrigin>>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Instance {
    pub unit: EffectUnit,
    pub bindings: Vec<(TypeId, TypeId)>,
    pub environment: Option<crate::facts::CallableEnvironmentId>,
    /// Invoking this unit advances a cursor; its returned tail must also be
    /// followed. Creating that tail alone does not execute its next callback.
    pub cursor: Option<TypeId>,
    pub values: ValueBindings,
}

impl Instance {
    pub fn declaration(unit: EffectUnit) -> Self {
        Self {
            unit,
            bindings: Vec::new(),
            environment: None,
            cursor: None,
            values: BTreeMap::new(),
        }
    }
}

pub(super) enum Witness {
    Source(Instance),
    External(etas_code::CallableId),
}

fn signature(context: CallContext<'_>, unit: EffectUnit) -> Option<&CallableSignature> {
    let EffectUnit::Item(item) = unit else {
        return None;
    };
    match context.types.facts.item_signatures.get(&item)? {
        ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => Some(signature),
        ItemSignature::Agent(signature) => signature.callable(),
        _ => None,
    }
}

pub(super) fn expression_type(context: CallContext<'_>, expression: HirExprId) -> Option<TypeId> {
    substitute_type(context, *context.types.facts.expr_types.get(&expression)?)
}

pub(super) fn substitute_type(context: CallContext<'_>, ty: TypeId) -> Option<TypeId> {
    let names = HashMap::new();
    let parameters = context.bindings.iter().copied().collect();
    TypeSubstitutionEngine::existing(&context.types.store, &names)
        .with_type_parameters(&parameters)
        .with_execution_control(context.control)
        .substitute(ty)
        .ok()
}

fn bind(
    context: CallContext<'_>,
    unit: EffectUnit,
    owner: &[(TypeId, TypeId)],
    parameters: &[CallableGenericParam],
    arguments: Option<&GenericInstantiationFact>,
) -> Option<Instance> {
    let target = signature(context, unit)?;
    let names = HashMap::new();
    let inherited = context.bindings.iter().copied().collect();
    let mut substitution = TypeSubstitutionEngine::existing(&context.types.store, &names)
        .with_type_parameters(&inherited)
        .with_execution_control(context.control);
    let mut bindings = HashMap::new();
    for (parameter, argument) in owner {
        bindings.insert(*parameter, substitution.substitute(*argument).ok()?);
    }
    if parameters.len() != target.generic_params.len() {
        return None;
    }
    for (source, parameter) in parameters.iter().zip(&target.generic_params) {
        if parameter.kind != CallableGenericParamKind::Type {
            continue;
        }
        let argument = arguments?
            .type_bindings
            .iter()
            .find(|(name, _)| name == &source.name)?
            .1;
        bindings.insert(parameter.subject, substitution.substitute(argument).ok()?);
    }
    let mut bindings = bindings
        .into_iter()
        .filter(|(parameter, argument)| parameter != argument)
        .collect::<Vec<_>>();
    bindings.sort_unstable();
    Some(Instance {
        unit,
        bindings,
        environment: None,
        cursor: None,
        values: context.values.clone(),
    })
}

pub(super) fn source(
    context: CallContext<'_>,
    call: HirExprId,
    unit: EffectUnit,
) -> Option<Instance> {
    if !matches!(unit, EffectUnit::Item(_)) {
        return Some(arguments(
            context,
            call,
            Instance {
                unit,
                bindings: context.bindings.to_vec(),
                environment: context.environment,
                cursor: None,
                values: context.values.clone(),
            },
        ));
    }
    if let Some(method) = context.types.facts.checked_method_calls.get(&call) {
        return bind(
            context,
            unit,
            &method.owner_bindings,
            &method.signature.generic_params,
            Some(&method.instantiation),
        )
        .map(|instance| arguments(context, call, instance));
    }
    let signature = signature(context, unit)?;
    bind(
        context,
        unit,
        &[],
        &signature.generic_params,
        context.types.facts.generic_instantiations.get(&call),
    )
    .map(|instance| arguments(context, call, instance))
}

pub(super) fn arguments(
    context: CallContext<'_>,
    call: HirExprId,
    mut instance: Instance,
) -> Instance {
    if let Some(arguments) = context.effects.source_call_arguments.get(&call) {
        for (target, sources) in arguments {
            instance.values.insert(*target, resolve(context, sources));
        }
    }
    let applied = CallContext {
        bindings: &instance.bindings,
        ..context
    };
    instance.values.retain(|target, _| {
        let ty = match target {
            crate::ParameterCallTarget::Native(symbol) => {
                match context.types.facts.symbol_types.get(symbol) {
                    Some(etas_types::SymbolTypeFact::Param { ty }) => Some(*ty),
                    _ => None,
                }
            }
            crate::ParameterCallTarget::Projection(id) => context
                .effects
                .parameter_projections
                .get(id)
                .map(|projection| projection.ty),
            _ => None,
        };
        ty.and_then(|ty| substitute_type(applied, ty))
            .is_none_or(|ty| {
                context.payload_shapes.contains(ty) || context.payload_shapes.has_parameters(ty)
            })
    });
    instance
}

/// Resolve only origins recorded at checked invocation/capture boundaries.
/// Unbound parameters and cycles remain symbolic, hence unknown on invocation.
pub(super) fn resolve(
    context: CallContext<'_>,
    sources: &BTreeSet<CallableValueOrigin>,
) -> BTreeSet<CallableValueOrigin> {
    let mut pending = sources
        .iter()
        .copied()
        .map(|source| (Some(source), None))
        .collect::<Vec<_>>();
    let mut active = BTreeSet::new();
    let mut expanded = BTreeSet::new();
    let mut out = BTreeSet::new();
    while let Some((source, leave)) = pending.pop() {
        if let Some(target) = leave {
            active.remove(&target);
            continue;
        }
        let source = source.expect("origin traversal entry");
        let target = match source {
            CallableValueOrigin::Parameter { symbol, .. } => {
                Some(crate::ParameterCallTarget::Native(symbol))
            }
            CallableValueOrigin::Projection(id) => Some(crate::ParameterCallTarget::Projection(id)),
            _ => None,
        };
        if let Some(target) = target {
            if active.contains(&target) {
                out.insert(source);
                continue;
            }
            if !expanded.insert(target) {
                continue;
            }
            let found = context.values.get(&target).or_else(|| {
                context
                    .environment
                    .and_then(|id| context.effects.callable_environments.get(&id))
                    .and_then(|snapshot| snapshot.parameters.get(&target))
            });
            if let Some(found) = found {
                active.insert(target);
                pending.push((None, Some(target)));
                pending.extend(found.iter().copied().map(|origin| (Some(origin), None)));
                continue;
            }
        }
        out.insert(source);
    }
    out
}

pub(super) fn captured(
    context: CallContext<'_>,
    mut instance: Instance,
    environment: CallableEnvironmentId,
) -> Option<Instance> {
    let snapshot = context.effects.callable_environments.get(&environment)?;
    let mut bindings = instance.bindings.iter().copied().collect::<HashMap<_, _>>();
    for (parameter, value) in &snapshot.generics {
        let CallableGenericParameter::Native { owner, position } = parameter else {
            return None;
        };
        let parameters = match context.types.facts.item_signatures.get(owner) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                &signature.generic_params
            }
            Some(ItemSignature::Agent(signature)) => &signature.callable()?.generic_params,
            _ => {
                &context
                    .types
                    .facts
                    .type_impl_methods
                    .values()
                    .find(|method| method.implementation == *owner)?
                    .owner_params
            }
        };
        let parameter = parameters.get(*position as usize)?;
        if parameter.kind == CallableGenericParamKind::Type {
            let CallableGenericValue::Type(value) = value else {
                return None;
            };
            bindings.insert(parameter.subject, substitute_type(context, *value)?);
        }
    }
    instance.bindings = bindings
        .into_iter()
        .filter(|(parameter, value)| parameter != value)
        .collect();
    instance.bindings.sort_unstable();
    for (parameter, origins) in &snapshot.parameters {
        instance
            .values
            .insert(*parameter, resolve(context, origins));
    }
    instance.environment = Some(environment);
    Some(instance)
}

pub(super) fn witness(
    context: CallContext<'_>,
    call: &CheckedMethodCallFact,
    member: &etas_types::SpecMethodIdentity,
) -> Option<Witness> {
    let MethodEvidence::Bound { args, .. } = &call.evidence else {
        return None;
    };
    let names = HashMap::new();
    let parameters = context.bindings.iter().copied().collect();
    let mut substitution = TypeSubstitutionEngine::existing(&context.types.store, &names)
        .with_type_parameters(&parameters)
        .with_execution_control(context.control);
    let receiver = substitution.substitute(call.receiver_type).ok()?;
    let args = args
        .iter()
        .map(|arg| substitution.substitute(*arg))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let witness = context
        .types
        .facts
        .methods
        .resolve_witness(&context.types.store, context.specs, receiver, &args, member)
        .ok()??;
    match &witness.declaration.target {
        MethodTarget::Source { callable, .. } => {
            let unit = EffectUnit::Item(*callable);
            if !context.effects.unit_effects.contains_key(&unit) {
                return None;
            }
            let owner = witness.owner_bindings.into_iter().collect::<Vec<_>>();
            bind(
                context,
                unit,
                &owner,
                &call.signature.generic_params,
                Some(&call.instantiation),
            )
            .map(Witness::Source)
        }
        MethodTarget::External { identity } => Some(Witness::External(
            etas_code::CallableId::method(identity.clone()),
        )),
        _ => None,
    }
}
