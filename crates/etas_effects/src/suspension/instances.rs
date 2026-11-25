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

/// A closure's checked return origins belong to its captured application.
/// The declaration summary is used only when no such application was analyzed.
pub(super) fn checked_summary(
    effects: &crate::EffectFacts,
    unit: EffectUnit,
    environment: Option<CallableEnvironmentId>,
) -> Option<&crate::EffectSummary> {
    if let Some(environment) = environment {
        match effects
            .captured_callables
            .get(&crate::facts::CapturedCallableId { unit, environment })
        {
            Some(crate::facts::CapturedCallableAnalysis::Solved(summary)) => {
                return Some(summary);
            }
            Some(crate::facts::CapturedCallableAnalysis::Unavailable { .. }) => return None,
            None => {}
        }
    }
    effects.unit_effects.get(&unit)
}

/// Checked invocation dependencies, distinct from merely carrying data that
/// might contain a callable. Grouping lexical expressions also covers captures
/// whose execution is analyzed as a separate anonymous unit.
pub(super) struct ValueInputs {
    parameters: BTreeMap<EffectUnit, BTreeSet<crate::ParameterCallTarget>>,
    origins: BTreeMap<etas_hir::HirItemId, BTreeSet<CallableValueOrigin>>,
}

impl ValueInputs {
    pub fn new(hir: &etas_hir::HirRevision, effects: &crate::EffectFacts) -> Self {
        let parameters = effects
            .unit_effects
            .iter()
            .map(|(unit, summary)| {
                (
                    *unit,
                    summary
                        .action_trace
                        .parameter_calls()
                        .into_iter()
                        .map(|call| call.target)
                        .collect(),
                )
            })
            .collect();
        let mut origins = BTreeMap::<_, BTreeSet<_>>::new();
        for (expr, sources) in effects
            .dynamic_call_sources
            .iter()
            .chain(&effects.iteration_call_sources)
        {
            if let Some(owner) = hir.index().expr_item.get(expr) {
                origins.entry(*owner).or_default().extend(sources);
            }
        }
        Self {
            parameters,
            origins,
        }
    }
}

/// Canonicalize the proof key using invoked parameters and their transitive
/// capture dependencies. A generic data field is not an invocation dependency.
pub(super) fn retain_value_dependencies(
    effects: &crate::EffectFacts,
    inputs: &ValueInputs,
    instance: &mut Instance,
) {
    use crate::ParameterCallTarget as Target;

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Dependency {
        Unit(EffectUnit),
        Parameter(Target),
        Environment(CallableEnvironmentId),
    }
    let owner = |unit| match unit {
        EffectUnit::Item(owner)
        | EffectUnit::AnonymousFlow { owner, .. }
        | EffectUnit::HandlerArm { owner, .. }
        | EffectUnit::ParameterDefault { owner, .. } => owner,
    };
    let mut pending = vec![Dependency::Unit(instance.unit)];
    if instance.cursor.is_some() {
        use crate::{CallableReturnSource as Return, ProjectedReturnSources};
        use etas_code::descriptor::ValueProjectionStep as Step;
        let tail = vec![
            Step::VariantPayload {
                variant: "Some".into(),
                position: 0,
            },
            Step::TupleElement(1),
            Step::Field("next".into()),
        ];
        if let Some(ProjectedReturnSources::Known(sources)) =
            checked_summary(effects, instance.unit, instance.environment)
                .and_then(|summary| summary.exits.as_ref())
                .and_then(|exits| exits.value_projections.get(&tail))
        {
            for source in sources {
                match source {
                    Return::Parameter(symbol) => {
                        pending.push(Dependency::Parameter(Target::Native(*symbol)))
                    }
                    Return::Projection(id) => {
                        pending.push(Dependency::Parameter(Target::Projection(*id)))
                    }
                    Return::SourceItem(item) => {
                        pending.push(Dependency::Unit(EffectUnit::Item(*item)))
                    }
                    Return::AnonymousFlow(unit) => pending.push(Dependency::Unit(*unit)),
                    Return::Captured { unit, environment } => {
                        pending.push(Dependency::Unit(*unit));
                        pending.push(Dependency::Environment(*environment));
                    }
                    Return::Reference(reference) => {
                        if let crate::CallableReferenceTarget::Source(item) = reference.target {
                            pending.push(Dependency::Unit(EffectUnit::Item(item)));
                        }
                        pending.extend(reference.environment.map(Dependency::Environment));
                    }
                    _ => {}
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut retained = BTreeSet::new();
    while let Some(dependency) = pending.pop() {
        if !seen.insert(dependency) {
            continue;
        }
        let sources = match dependency {
            Dependency::Unit(unit) => {
                pending.extend(
                    inputs
                        .parameters
                        .get(&unit)
                        .into_iter()
                        .flatten()
                        .copied()
                        .map(Dependency::Parameter),
                );
                inputs
                    .origins
                    .get(&owner(unit))
                    .into_iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>()
            }
            Dependency::Parameter(target) => {
                retained.insert(target);
                let mut sources = instance
                    .values
                    .get(&target)
                    .or_else(|| {
                        instance
                            .environment
                            .and_then(|id| effects.callable_environments.get(&id))
                            .and_then(|snapshot| snapshot.parameters.get(&target))
                    })
                    .into_iter()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>();
                if let Target::Projection(id) = target {
                    if let Some(projection) = effects.parameter_projections.get(&id) {
                        pending.push(Dependency::Parameter(Target::Native(projection.parameter)));
                        for inputs in projection.call_inputs.values() {
                            for argument in &inputs.arguments {
                                sources.extend(&argument.sources);
                                sources.extend(
                                    argument
                                        .projections
                                        .iter()
                                        .flat_map(|projection| &projection.sources),
                                );
                            }
                        }
                    }
                }
                sources
            }
            Dependency::Environment(environment) => effects
                .callable_environments
                .get(&environment)
                .into_iter()
                .flat_map(|snapshot| snapshot.parameters.values().flatten())
                .copied()
                .collect(),
        };
        for source in sources {
            match source {
                CallableValueOrigin::Parameter { symbol, .. } => {
                    pending.push(Dependency::Parameter(Target::Native(symbol)));
                }
                CallableValueOrigin::Projection(id) => {
                    pending.push(Dependency::Parameter(Target::Projection(id)));
                }
                CallableValueOrigin::Unit(unit) => pending.push(Dependency::Unit(unit)),
                CallableValueOrigin::Captured { unit, environment } => {
                    pending.push(Dependency::Unit(unit));
                    pending.push(Dependency::Environment(environment));
                }
                CallableValueOrigin::Reference {
                    unit, environment, ..
                }
                | CallableValueOrigin::FrozenContract {
                    unit, environment, ..
                } => {
                    pending.push(Dependency::Unit(unit));
                    pending.extend(environment.map(Dependency::Environment));
                }
                CallableValueOrigin::StdReference { environment, .. } => {
                    pending.extend(environment.map(Dependency::Environment));
                }
                CallableValueOrigin::Existential { environment, .. }
                | CallableValueOrigin::SpecMethod { environment, .. }
                | CallableValueOrigin::MetadataSpecMethod { environment, .. } => {
                    pending.push(Dependency::Environment(environment));
                }
                _ => {}
            }
        }
    }
    instance
        .values
        .retain(|target, _| retained.contains(target));
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
    TypeSubstitutionEngine::existing_indexed(&context.types.store, context.type_lookup, &names)
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
    let mut substitution =
        TypeSubstitutionEngine::existing_indexed(&context.types.store, context.type_lookup, &names)
            .with_type_parameters(&inherited)
            .with_execution_control(context.control);
    // A captured callable can still refer to its caller's type binders after
    // this call introduces another declaration scope.
    let mut bindings = inherited.clone();
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
    let mut substitution =
        TypeSubstitutionEngine::existing_indexed(&context.types.store, context.type_lookup, &names)
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
        .ok()?;
    let witness = witness?;
    match &witness.declaration.target {
        MethodTarget::Source { callable, .. } => {
            let unit = EffectUnit::Item(*callable);
            if !context.effects.unit_effects.contains_key(&unit) {
                return None;
            }
            let owner = witness.owner_bindings.into_iter().collect::<Vec<_>>();
            let bound = bind(
                context,
                unit,
                &owner,
                &call.signature.generic_params,
                Some(&call.instantiation),
            );
            bound.map(Witness::Source)
        }
        MethodTarget::External { identity } => Some(Witness::External(
            etas_code::CallableId::method(identity.clone()),
        )),
        _ => None,
    }
}

/// Select the callable position before an invocation, keeping its checked
/// argument recipe for the eventual concrete implementation.
pub(super) fn application(
    context: CallContext<'_>,
    source: CallableValueOrigin,
) -> Option<(
    BTreeSet<CallableValueOrigin>,
    u32,
    std::sync::Arc<crate::facts::NativeCallInputs>,
)> {
    use crate::ParameterCallTarget as Target;
    use etas_code::descriptor::ValueProjectionStep as Step;
    let CallableValueOrigin::Projection(id) = source else {
        return None;
    };
    let projection = context.effects.parameter_projections.get(&id)?;
    let [prefix @ .., Step::Invocation { inputs }] = projection.path.as_slice() else {
        return None;
    };
    let root = Target::Native(projection.parameter);
    let selected = context
        .values
        .iter()
        .chain(
            context
                .environment
                .and_then(|id| context.effects.callable_environments.get(&id))
                .into_iter()
                .flat_map(|snapshot| &snapshot.parameters),
        )
        .find_map(|(target, sources)| {
            let found = if prefix.is_empty() {
                *target == root
            } else {
                match target {
                    Target::Projection(id) => context
                        .effects
                        .parameter_projections
                        .get(id)
                        .is_some_and(|candidate| {
                            candidate.parameter == projection.parameter && candidate.path == prefix
                        }),
                    _ => false,
                }
            };
            found.then_some(sources)
        })?;
    let sources = resolve(context, selected);
    if sources.is_empty() {
        return None;
    }
    Some((
        sources,
        *inputs,
        projection.call_inputs.get(inputs)?.clone(),
    ))
}

pub(super) fn application_arguments(
    context: CallContext<'_>,
    inputs: &crate::facts::NativeCallInputs,
    mut instance: Instance,
    captured: Option<CallableEnvironmentId>,
) -> Option<Instance> {
    use crate::ParameterCallTarget as Target;
    let parameters = match instance.unit {
        EffectUnit::Item(item) => match context.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => &flow.params,
            etas_hir::HirItem::Tool(tool) => &tool.params,
            etas_hir::HirItem::Agent(agent) => &agent.params,
            _ => return None,
        },
        EffectUnit::AnonymousFlow { value, .. } => match context.hir.exprs.get(value)? {
            etas_hir::HirExpr::Lambda { params, .. } => params,
            _ => return None,
        },
        _ => return None,
    };
    let parameters = parameters
        .iter()
        .copied()
        .filter(|parameter| {
            !captured
                .and_then(|id| context.effects.callable_environments.get(&id))
                .is_some_and(|snapshot| {
                    snapshot
                        .parameters
                        .contains_key(&Target::Native(*parameter))
                })
        })
        .collect::<Vec<_>>();
    if parameters.len() != inputs.arguments.len() {
        return None;
    }
    let positional = inputs
        .arguments
        .iter()
        .filter(|argument| argument.name.is_none())
        .collect::<Vec<_>>();
    let mut position = 0;
    for parameter in parameters {
        let symbol = context.hir.symbols.get(parameter)?;
        let argument = inputs
            .arguments
            .iter()
            .find(|argument| argument.name.as_ref() == Some(&symbol.name))
            .or_else(|| {
                let found = positional.get(position).copied();
                position += 1;
                found
            })?;
        instance.values.insert(
            Target::Native(parameter),
            resolve(context, &argument.sources),
        );
        let etas_hir::SymbolDef::Param { owner, .. } = symbol.def else {
            return None;
        };
        for projected in &argument.projections {
            let id = crate::facts::NativeParameterProjection::path_identity(
                owner,
                parameter,
                &projected.path,
            )
            .ok()?;
            if context.effects.parameter_projections.contains_key(&id) {
                instance
                    .values
                    .insert(Target::Projection(id), resolve(context, &projected.sources));
            }
        }
    }
    Some(instance)
}
