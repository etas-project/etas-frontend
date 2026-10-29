use etas_code::descriptor::{
    CallableOutcomeInfo, DescriptorGraph, ParameterArgumentInfo, ParameterCallInfo,
    ParameterHandlerScopeInfo, ReturnObservationInfo, TraceInfo,
};
use etas_types::{PrimitiveType, Type};

use super::{engine::EffectSemantics, shared::*};
use crate::{
    CallableExitSummary, CallableOutcome, EffectRow, ExternalCallableContract, OutcomeAnalysisGap,
    ParameterArgumentValue, ParameterCallTarget, ParameterExitArgument, ParameterExitFact,
    ParameterHandlerScopeRef, RequestInstantiationSummary, ReturnObservation,
};

impl EffectSemantics<'_> {
    /// Replays only outcome branches whose portable control flow and checked
    /// callback interface can be proved exactly. Other shapes remain unknown.
    pub(super) fn external_callback_outcomes(
        &self,
        contract: &ExternalCallableContract,
        summary: &EffectSummary,
        signature: &etas_types::FlowType,
        span: Span,
    ) -> Option<CallableExitSummary> {
        let graph =
            DescriptorGraph::new(contract.graph.as_ref().clone(), Default::default()).ok()?;
        let RequestInstantiationSummary::Available {
            actions,
            parameters,
        } = &summary.handler_requests
        else {
            return None;
        };
        let mut candidates = crate::EffectSet::new();
        for row in [
            &summary.escaping_effects,
            &summary.requested_actions,
            &summary.default_actions,
            &summary.handled_actions,
        ] {
            candidates.union_assign(&row.effects);
        }
        for parameter in parameters {
            candidates.union_assign(&parameter.excluded);
        }
        for action in actions {
            candidates.insert(action.action.clone());
        }
        for ty in &signature.input {
            if let Some(Type::Function(callback)) = self.types.store.get(*ty)
                && let Some(row) = callback.effects.as_ref()
            {
                candidates.union_assign(&self.row_from_type_ref(row).effects);
            }
        }
        let checked_row = |reference| -> Option<EffectRow> {
            let row = graph.effect_row(reference)?;
            if row.tail.is_some() {
                return None;
            }
            let mut effects = crate::EffectSet::new();
            for effect in &row.effects {
                effects.insert(self.checked_descriptor_action(
                    &graph,
                    contract,
                    effect,
                    &candidates,
                )?);
            }
            Some(EffectRow::closed(effects))
        };
        if checked_row(contract.header.analysis.effects.escaping?)? != summary.escaping_effects
            || checked_row(contract.header.analysis.effects.requested?)?
                != summary.requested_actions
            || checked_row(contract.header.analysis.effects.default_handled?)?
                != summary.default_actions
        {
            return None;
        }
        let independent = match contract.header.analysis.independent_determinism? {
            etas_code::descriptor::DeterminismInfo::Deterministic => {
                crate::Determinism::Deterministic
            }
            etas_code::descriptor::DeterminismInfo::RuntimeMediated => {
                crate::Determinism::RuntimeMediated
            }
            etas_code::descriptor::DeterminismInfo::NonDeterministic => {
                crate::Determinism::NonDeterministic
            }
        };
        if summary.independent_determinism != Some(independent) {
            return None;
        }
        let trace_root = contract.header.analysis.trace?;
        let outcomes_root = contract.header.analysis.outcomes?;
        let mut budget = etas_code::descriptor::DescriptorLimits::default().max_edges;
        let paths = descriptor_call_paths(&graph, trace_root, &mut budget)?;
        let native_paths = native_call_paths(&summary.action_trace, &mut budget)?;
        if paths.is_empty() || paths.len() != native_paths.len() {
            return None;
        }
        let mut calls = std::collections::BTreeMap::new();
        for (path, native) in paths.iter().zip(&native_paths) {
            if path.len() != native.len() {
                return None;
            }
            for (reference, native_call) in path.iter().zip(native) {
                let TraceInfo::ParameterCall(call) = graph.trace(*reference)? else {
                    return None;
                };
                if calls
                    .insert(*reference, (call, *native_call))
                    .is_some_and(|(_, previous)| previous != *native_call)
                {
                    return None;
                }
            }
        }
        if calls.is_empty() {
            return None;
        }
        let etas_code::descriptor::TypeInfo::Function(portable_signature) =
            graph.type_info(contract.header.signature)?
        else {
            return None;
        };
        let native_output = self.types.store.get(signature.output)?;
        if !etas_types::checked_descriptor_type_matches(
            self.types,
            &graph,
            signature.output,
            portable_signature.output,
        )
        .ok()?
        {
            return None;
        }
        if summary.requested_actions.open.is_some() {
            return None;
        }
        let source: crate::FrozenSourceLocation = span.into();
        let mut requested = crate::EffectSet::new();
        let mut checked_calls = std::collections::BTreeMap::new();
        for (reference, (root_call, native_call)) in calls {
            if root_call.callable != contract.header.id
                || root_call.signature != contract.header.signature
            {
                return None;
            }
            let target = ParameterCallTarget::Interface(root_call.position);
            if !matches!(native_call, ActionTraceDomain::ParameterCall { target: actual, .. } if *actual == target)
            {
                return None;
            }
            let Type::Function(callback) = self
                .types
                .store
                .get(*signature.input.get(root_call.position as usize)?)?
            else {
                return None;
            };
            let portable_parameter = portable_signature
                .parameters
                .get(root_call.position as usize)?;
            let etas_code::descriptor::TypeInfo::Function(portable_callback) =
                graph.type_info(portable_parameter.ty)?
            else {
                return None;
            };
            if portable_callback.parameters.len() != callback.input.len()
                || !etas_types::checked_descriptor_type_matches(
                    self.types,
                    &graph,
                    callback.output,
                    portable_callback.output,
                )
                .ok()?
            {
                return None;
            }
            let arguments = root_call.arguments.as_ref()?;
            if arguments.len() != callback.input.len() {
                return None;
            }
            let arguments = arguments
                .iter()
                .zip(&callback.input)
                .zip(&portable_callback.parameters)
                .map(|((argument, ty), portable)| {
                    if !etas_types::checked_descriptor_type_matches(
                        self.types,
                        &graph,
                        *ty,
                        portable.ty,
                    )
                    .ok()?
                    {
                        return None;
                    }
                    let value = match argument {
                        ParameterArgumentInfo::Bool(value) => ParameterArgumentValue::Bool(*value),
                        ParameterArgumentInfo::Integer {
                            negative,
                            magnitude,
                        } => ParameterArgumentValue::Integer {
                            negative: *negative,
                            magnitude: *magnitude,
                        },
                        ParameterArgumentInfo::Input { position } => {
                            let owner_ty = *signature.input.get(*position as usize)?;
                            let portable_owner =
                                portable_signature.parameters.get(*position as usize)?;
                            if owner_ty != *ty
                                || graph.type_fingerprint(portable_owner.ty).ok()?
                                    != graph.type_fingerprint(portable.ty).ok()?
                            {
                                return None;
                            }
                            ParameterArgumentValue::Input {
                                position: *position,
                            }
                        }
                    };
                    Some(ParameterExitArgument {
                        name: None,
                        ty: *ty,
                        source: source.clone(),
                        value: Some(value),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let callback_row = self.row_from_type_ref(callback.effects.as_ref()?);
            if checked_row(portable_callback.effects?)? != callback_row {
                return None;
            }
            let handlers = root_call.handlers.as_ref()?;
            if callback_row.open.is_some() {
                return None;
            }
            requested.union_assign(&callback_row.effects);
            let mut handled = crate::EffectSet::new();
            for scope in handlers {
                let ParameterHandlerScopeInfo::Handle { arms } = scope;
                for arm in arms {
                    handled.insert(self.checked_descriptor_action(
                        &graph,
                        contract,
                        &arm.action,
                        &candidates,
                    )?);
                    let mut pending = vec![arm.trace];
                    let mut seen = std::collections::BTreeSet::new();
                    while let Some(trace) = pending.pop() {
                        if !seen.insert(trace.0) {
                            continue;
                        }
                        match graph.trace(trace)? {
                            TraceInfo::Empty => {}
                            TraceInfo::Event(action) => {
                                requested.insert(self.checked_descriptor_action(
                                    &graph,
                                    contract,
                                    action,
                                    &candidates,
                                )?);
                            }
                            TraceInfo::Seq(parts) | TraceInfo::Choice(parts) => {
                                pending.extend(parts.iter().copied())
                            }
                            TraceInfo::Repeat(inner) => pending.push(*inner),
                            _ => return None,
                        }
                    }
                }
            }
            if !parameters
                .iter()
                .any(|parameter| parameter.target == target && parameter.excluded == handled)
            {
                return None;
            }
            let scopes = handlers
                .iter()
                .enumerate()
                .map(|(scope, _)| {
                    Some(ParameterHandlerScopeRef::External {
                        graph: contract.descriptor_fingerprint,
                        parameter_call: reference.0,
                        call_index: 0,
                        scope: u32::try_from(scope).ok()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            checked_calls.insert(
                reference,
                (target, callback.output, arguments, handled, scopes),
            );
        }
        if requested != summary.requested_actions.effects
            || parameters.iter().any(|parameter| {
                !checked_calls.values().any(|(target, _, _, handled, _)| {
                    *target == parameter.target && *handled == parameter.excluded
                })
            })
        {
            return None;
        }
        let TraceInfo::Choice(branches) = graph.trace(outcomes_root)? else {
            return None;
        };
        let mut exits = CallableExitSummary::default();
        let mut callback_exit = false;
        for branch in branches {
            let TraceInfo::Seq(parts) = graph.trace(*branch)? else {
                return None;
            };
            let [prefix, terminal] = parts.as_slice() else {
                return None;
            };
            match graph.trace(*terminal)? {
                TraceInfo::Outcome(CallableOutcomeInfo::Returned { ty, observation }) => {
                    if graph.type_fingerprint(*ty).ok()?
                        != graph.type_fingerprint(portable_signature.output).ok()?
                        || graph.trace_fingerprint(*prefix).ok()?
                            != graph.trace_fingerprint(trace_root).ok()?
                    {
                        return None;
                    }
                    let observation = match (native_output, observation) {
                        (_, ReturnObservationInfo::Any) => ReturnObservation::Any,
                        (Type::Primitive(PrimitiveType::Unit), ReturnObservationInfo::Unit) => {
                            ReturnObservation::Unit
                        }
                        (
                            Type::Primitive(PrimitiveType::Bool),
                            ReturnObservationInfo::Bool(value),
                        ) => ReturnObservation::Bool(*value),
                        _ => return None,
                    };
                    exits.record(
                        CallableOutcome::Returned {
                            ty: signature.output,
                            observation,
                        },
                        summary.action_trace.clone(),
                    );
                }
                TraceInfo::Outcome(CallableOutcomeInfo::ParameterNonReturn(call)) => {
                    for preceding in descriptor_call_paths(&graph, *prefix, &mut budget)? {
                        let position = preceding.len();
                        let mut matched = false;
                        for (path, native_path) in paths.iter().zip(&native_paths) {
                            budget = budget.checked_sub(position.saturating_add(1))?;
                            if position >= path.len() {
                                continue;
                            }
                            let mut equivalent = true;
                            for (actual, expected) in preceding.iter().zip(path) {
                                let (
                                    Some(TraceInfo::ParameterCall(actual)),
                                    Some(TraceInfo::ParameterCall(expected)),
                                ) = (graph.trace(*actual), graph.trace(*expected))
                                else {
                                    return None;
                                };
                                if !same_call(&graph, actual, expected) {
                                    equivalent = false;
                                    break;
                                }
                            }
                            let Some(TraceInfo::ParameterCall(expected)) =
                                graph.trace(path[position])
                            else {
                                return None;
                            };
                            if !equivalent || !same_call(&graph, expected, call) {
                                continue;
                            }
                            matched = true;
                            let (target, output, arguments, excluded, scopes) =
                                checked_calls.get(&path[position])?;
                            let mut prefix = ActionTraceDomain::Empty;
                            for call in &native_path[..position] {
                                prefix.seq_assign((*call).clone());
                            }
                            exits.gaps.insert(OutcomeAnalysisGap::ParameterCall {
                                target: *target,
                                output: *output,
                                source: source.clone(),
                            });
                            exits.record_parameter_exit(ParameterExitFact {
                                target: *target,
                                output: *output,
                                source: source.clone(),
                                arguments: Some(arguments.clone()),
                                handler_free: Some(scopes.is_empty()),
                                enclosing_handled: Some(EffectRow::closed(excluded.clone())),
                                enclosing_handlers: Some(scopes.clone()),
                                prefix,
                            });
                            callback_exit = true;
                        }
                        if !matched {
                            return None;
                        }
                    }
                }
                _ => return None,
            }
        }
        (!exits.exits.is_empty() && callback_exit).then_some(exits)
    }

    fn checked_descriptor_action(
        &self,
        graph: &DescriptorGraph,
        contract: &ExternalCallableContract,
        effect: &etas_code::descriptor::EffectInfo,
        candidates: &crate::EffectSet,
    ) -> Option<crate::Effect> {
        let etas_code::descriptor::EffectTargetInfo::Action {
            effect: identity,
            name,
        } = &effect.target
        else {
            return None;
        };
        let mut resolved = None;
        for path in contract.definition_paths.get(identity)? {
            let mut path = path.clone();
            path.push(name.clone());
            let Some(action) = self.registry.action_by_name(&path.join(".")) else {
                continue;
            };
            if self
                .registry
                .action_signature(&action)
                .is_none_or(|signature| signature.effect_args.len() != effect.arguments.len())
            {
                return None;
            }
            if resolved.as_ref().is_some_and(|prior| prior != &action) {
                return None;
            }
            resolved = Some(action);
        }
        let action = resolved?;
        if effect.arguments.is_empty() {
            return Some(crate::Effect::Action(action));
        }
        let mut matching = candidates.iter().filter(|candidate| {
            let crate::Effect::AppliedAction(instance) = candidate else {
                return false;
            };
            instance.action == action
                && instance.args.len() == effect.arguments.len()
                && effect
                    .arguments
                    .iter()
                    .zip(&instance.args)
                    .all(|(portable, native)| {
                        use etas_code::descriptor::StaticEffectArgInfo as S;
                        match (portable, native) {
                            (S::Type(reference), EffectArgRef::Type(ty)) => {
                                etas_types::checked_descriptor_type_matches(
                                    self.types, graph, *ty, *reference,
                                )
                                .unwrap_or(false)
                            }
                            (S::String(left), EffectArgRef::String(right))
                            | (S::Integer(left), EffectArgRef::Int(right)) => left == right,
                            (S::Wildcard, EffectArgRef::Wildcard) => true,
                            (S::Resource(identity), EffectArgRef::Path(path)) => contract
                                .definition_paths
                                .get(identity)
                                .is_some_and(|paths| paths.contains(path)),
                            (S::Callable(identity), EffectArgRef::Path(path)) => identity
                                .as_declaration()
                                .and_then(|identity| contract.definition_paths.get(identity))
                                .is_some_and(|paths| paths.contains(path)),
                            _ => false,
                        }
                    })
        });
        let result = matching.next()?.clone();
        matching.next().is_none().then_some(result)
    }
}

fn descriptor_call_paths(
    graph: &DescriptorGraph,
    root: etas_code::descriptor::TraceInfoRef,
    budget: &mut usize,
) -> Option<Vec<Vec<etas_code::descriptor::TraceInfoRef>>> {
    finite_call_paths(root, budget, |reference| match graph.trace(*reference)? {
        TraceInfo::Empty => Some(CallPathNode::Empty),
        TraceInfo::Seq(parts) => Some(CallPathNode::Sequence(parts.clone())),
        TraceInfo::Choice(parts) => Some(CallPathNode::Choice(parts.clone())),
        TraceInfo::ParameterCall(_) => Some(CallPathNode::Call(*reference)),
        _ => None,
    })
}

fn native_call_paths<'a>(
    root: &'a ActionTraceDomain,
    budget: &mut usize,
) -> Option<Vec<Vec<&'a ActionTraceDomain>>> {
    finite_call_paths(root, budget, |trace| match trace {
        ActionTraceDomain::Empty => Some(CallPathNode::Empty),
        ActionTraceDomain::Seq(parts) => Some(CallPathNode::Sequence(parts.iter().collect())),
        ActionTraceDomain::Choice(parts) => Some(CallPathNode::Choice(parts.iter().collect())),
        ActionTraceDomain::ParameterCall { .. } => Some(CallPathNode::Call(*trace)),
        _ => None,
    })
}

enum CallPathNode<T> {
    Empty,
    Call(T),
    Sequence(Vec<T>),
    Choice(Vec<T>),
}

fn finite_call_paths<T: Clone>(
    root: T,
    budget: &mut usize,
    mut node: impl FnMut(&T) -> Option<CallPathNode<T>>,
) -> Option<Vec<Vec<T>>> {
    enum Step<T> {
        Visit(T),
        Sequence(usize),
        Choice(usize),
    }
    let limits = etas_code::descriptor::DescriptorLimits::default();
    let mut pending = vec![Step::Visit(root)];
    let mut values = Vec::<Vec<Vec<T>>>::new();
    while let Some(step) = pending.pop() {
        *budget = budget.checked_sub(1)?;
        match step {
            Step::Visit(value) => match node(&value)? {
                CallPathNode::Empty => values.push(vec![vec![]]),
                CallPathNode::Call(value) => values.push(vec![vec![value]]),
                CallPathNode::Sequence(parts) => {
                    pending.push(Step::Sequence(parts.len()));
                    pending.extend(parts.into_iter().rev().map(Step::Visit));
                }
                CallPathNode::Choice(parts) => {
                    pending.push(Step::Choice(parts.len()));
                    pending.extend(parts.into_iter().rev().map(Step::Visit));
                }
            },
            Step::Sequence(length) => {
                let start = values.len().checked_sub(length)?;
                let parts = values.drain(start..).collect::<Vec<_>>();
                let mut combined = vec![vec![]];
                for part in parts {
                    if combined.len().checked_mul(part.len())? > limits.max_nodes {
                        return None;
                    }
                    let mut next = Vec::new();
                    for left in &combined {
                        for right in &part {
                            let size = left.len().checked_add(right.len())?;
                            *budget = budget.checked_sub(size.saturating_add(1))?;
                            let mut path = Vec::with_capacity(size);
                            path.extend_from_slice(left);
                            path.extend_from_slice(right);
                            next.push(path);
                        }
                    }
                    combined = next;
                }
                values.push(combined);
            }
            Step::Choice(length) => {
                let start = values.len().checked_sub(length)?;
                let mut combined = Vec::new();
                for part in values.drain(start..) {
                    if combined.len().checked_add(part.len())? > limits.max_nodes {
                        return None;
                    }
                    combined.extend(part);
                }
                values.push(combined);
            }
        }
    }
    (values.len() == 1).then(|| values.pop()).flatten()
}

fn same_call(graph: &DescriptorGraph, left: &ParameterCallInfo, right: &ParameterCallInfo) -> bool {
    if left.callable != right.callable
        || left.signature != right.signature
        || left.position != right.position
        || left.arguments != right.arguments
    {
        return false;
    }
    let (Some(left), Some(right)) = (&left.handlers, &right.handlers) else {
        return false;
    };
    left.len() == right.len() && left.iter().zip(right).all(|(left, right)| {
        let (ParameterHandlerScopeInfo::Handle { arms: left }, ParameterHandlerScopeInfo::Handle { arms: right }) = (left, right);
        left.len() == right.len() && left.iter().zip(right).all(|(left, right)| {
            left.action == right.action && left.completion == right.completion &&
                matches!((graph.trace_fingerprint(left.trace), graph.trace_fingerprint(right.trace)), (Ok(left), Ok(right)) if left == right)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_code::descriptor::{
        BuiltinTypeInfo, DescriptorGraphInput, PrimitiveInfo, TypeInfo, TypeInfoRef,
    };

    #[test]
    fn checked_selector_type_comparison_preserves_nested_structure() {
        let mut store = etas_types::TypeStore::new();
        let i32_ty = store.intern(Type::Primitive(PrimitiveType::I32));
        let u32_ty = store.intern(Type::Primitive(PrimitiveType::U32));
        let list = store.intern(Type::List(i32_ty));
        let map = store.intern(Type::Map {
            key: u32_ty,
            value: list,
        });
        let mismatched = store.intern(Type::Map {
            key: i32_ty,
            value: list,
        });
        let graph = DescriptorGraph::new(
            DescriptorGraphInput {
                types: vec![
                    TypeInfo::Primitive(PrimitiveInfo::I32),
                    TypeInfo::Primitive(PrimitiveInfo::U32),
                    TypeInfo::Builtin {
                        constructor: BuiltinTypeInfo::List,
                        arguments: vec![TypeInfoRef(0)],
                    },
                    TypeInfo::Builtin {
                        constructor: BuiltinTypeInfo::Map,
                        arguments: vec![TypeInfoRef(1), TypeInfoRef(2)],
                    },
                ],
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
        let types = etas_types::TypeOutput {
            store: store.into(),
            ..Default::default()
        };
        assert!(
            etas_types::checked_descriptor_type_matches(&types, &graph, map, TypeInfoRef(3))
                .unwrap()
        );
        assert!(
            !etas_types::checked_descriptor_type_matches(
                &types,
                &graph,
                mismatched,
                TypeInfoRef(3)
            )
            .unwrap()
        );
        assert!(
            !etas_types::checked_descriptor_type_matches(&types, &graph, list, TypeInfoRef(3))
                .unwrap()
        );
    }
}
