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
        let calls = ordered_descriptor_calls(&graph, trace_root)?;
        let native_calls = ordered_native_calls(&summary.action_trace)?;
        if calls.is_empty() || calls.len() != native_calls.len() {
            return None;
        }
        let etas_code::descriptor::TypeInfo::Function(portable_signature) =
            graph.type_info(contract.header.signature)?
        else {
            return None;
        };
        let native_output = self.types.store.get(signature.output)?;
        if !same_primitive(native_output, graph.type_info(portable_signature.output)?) {
            return None;
        }
        if summary.requested_actions.open.is_some() {
            return None;
        }
        let source: crate::FrozenSourceLocation = span.into();
        let mut requested = crate::EffectSet::new();
        let mut checked_calls = Vec::with_capacity(calls.len());
        for ((reference, root_call), native_call) in calls.iter().zip(&native_calls) {
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
                || !same_primitive(
                    self.types.store.get(callback.output)?,
                    graph.type_info(portable_callback.output)?,
                )
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
                    if !same_primitive(self.types.store.get(*ty)?, graph.type_info(portable.ty)?) {
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
            checked_calls.push((target, callback.output, arguments, handled, scopes));
        }
        if requested != summary.requested_actions.effects
            || parameters.iter().any(|parameter| {
                !checked_calls.iter().any(|(target, _, _, handled, _)| {
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
                    let preceding = ordered_descriptor_calls(&graph, *prefix)?;
                    let position = preceding.len();
                    if position >= calls.len()
                        || preceding
                            .iter()
                            .zip(&calls)
                            .any(|((_, actual), (_, expected))| {
                                !same_call(&graph, actual, expected)
                            })
                        || !same_call(&graph, calls[position].1, call)
                    {
                        return None;
                    }
                    let (target, output, arguments, excluded, scopes) = &checked_calls[position];
                    let mut prefix = ActionTraceDomain::Empty;
                    for call in &native_calls[..position] {
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
                                checked_selector_type_matches(
                                    &self.types.store,
                                    graph,
                                    *ty,
                                    *reference,
                                )
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

fn checked_selector_type_matches(
    store: &etas_types::TypeStore,
    graph: &DescriptorGraph,
    native: etas_types::TypeId,
    portable: etas_code::descriptor::TypeInfoRef,
) -> bool {
    use etas_code::descriptor::{BuiltinTypeInfo as B, TypeInfo};
    use etas_types::{ResourceHandleType, TrustWrapper};
    let mut pending = vec![(native, portable)];
    let mut seen = std::collections::HashSet::new();
    let limit = etas_code::descriptor::DescriptorLimits::default().max_edges;
    let mut visited = 0usize;
    while let Some((native_id, portable_id)) = pending.pop() {
        visited += 1;
        if visited > limit {
            return false;
        }
        if !seen.insert((native_id, portable_id)) {
            continue;
        }
        let (Some(native), Some(portable)) = (store.get(native_id), graph.type_info(portable_id))
        else {
            return false;
        };
        if let TypeInfo::Alias { target, .. } = portable {
            pending.push((native_id, *target));
            continue;
        }
        if same_primitive(native, portable) {
            continue;
        }
        let (constructor, native_args) = match native {
            Type::Array(t) => (B::Array, vec![*t]),
            Type::List(t) => (B::List, vec![*t]),
            Type::Map { key, value } => (B::Map, vec![*key, *value]),
            Type::Set(t) => (B::Set, vec![*t]),
            Type::Range { index } => (B::Range, vec![*index]),
            Type::Slice(t) => (B::Slice, vec![*t]),
            Type::Option(t) => (B::Option, vec![*t]),
            Type::Result { ok, err } => (B::Result, vec![*ok, *err]),
            Type::Schema(t) => (B::Schema, vec![*t]),
            Type::Prompt => (B::Prompt, vec![]),
            Type::PromptPart => (B::PromptPart, vec![]),
            Type::Message(t) => (B::Message, vec![*t]),
            Type::MemorySelection(t) => (B::MemorySelection, vec![*t]),
            Type::Store { key, value } => (B::Store, vec![*key, *value]),
            Type::MemoryRegion(t) => (B::MemoryRegion, vec![*t]),
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                (B::MemoryRegionHandle, vec![*schema])
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                (B::ExternalToolHandle, vec![*signature])
            }
            Type::Trust { wrapper, inner } => (
                match wrapper {
                    TrustWrapper::Trusted => B::Trusted,
                    TrustWrapper::Untrusted => B::Untrusted,
                    TrustWrapper::Secret => B::Secret,
                    TrustWrapper::Public => B::Public,
                    TrustWrapper::Sanitized => B::Sanitized,
                },
                vec![*inner],
            ),
            Type::Tuple(elements) => {
                let TypeInfo::Tuple(other) = portable else {
                    return false;
                };
                if elements.len() != other.len() {
                    return false;
                }
                pending.extend(elements.iter().copied().zip(other.iter().copied()));
                continue;
            }
            Type::Record(record) => {
                let TypeInfo::Record(fields) = portable else {
                    return false;
                };
                if record.fields.len() != fields.len() {
                    return false;
                }
                for (left, right) in record.fields.iter().zip(fields) {
                    if left.name != right.name {
                        return false;
                    }
                    pending.push((left.ty, right.ty));
                }
                continue;
            }
            _ => return false,
        };
        let TypeInfo::Builtin {
            constructor: actual,
            arguments,
        } = portable
        else {
            return false;
        };
        if constructor != *actual || native_args.len() != arguments.len() {
            return false;
        }
        pending.extend(native_args.into_iter().zip(arguments.iter().copied()));
    }
    true
}

fn ordered_descriptor_calls(
    graph: &DescriptorGraph,
    root: etas_code::descriptor::TraceInfoRef,
) -> Option<Vec<(etas_code::descriptor::TraceInfoRef, &ParameterCallInfo)>> {
    let mut pending = vec![root];
    let mut calls = Vec::new();
    let mut visited = 0usize;
    let limit = etas_code::descriptor::DescriptorLimits::default().max_edges;
    while let Some(reference) = pending.pop() {
        visited = visited.checked_add(1)?;
        if visited > limit {
            return None;
        }
        match graph.trace(reference)? {
            TraceInfo::Empty => {}
            TraceInfo::Seq(parts) => pending.extend(parts.iter().rev().copied()),
            TraceInfo::ParameterCall(call) => calls.push((reference, call)),
            _ => return None,
        }
    }
    Some(calls)
}

fn ordered_native_calls(root: &ActionTraceDomain) -> Option<Vec<&ActionTraceDomain>> {
    let mut pending = vec![root];
    let mut calls = Vec::new();
    let mut visited = 0usize;
    let limit = etas_code::descriptor::DescriptorLimits::default().max_edges;
    while let Some(trace) = pending.pop() {
        visited = visited.checked_add(1)?;
        if visited > limit {
            return None;
        }
        match trace {
            ActionTraceDomain::Empty => {}
            ActionTraceDomain::Seq(parts) => pending.extend(parts.iter().rev()),
            ActionTraceDomain::ParameterCall { .. } => calls.push(trace),
            _ => return None,
        }
    }
    Some(calls)
}

fn same_primitive(native: &Type, portable: &etas_code::descriptor::TypeInfo) -> bool {
    use etas_code::descriptor::{PrimitiveInfo as P, TypeInfo};
    matches!(
        (native, portable),
        (
            Type::Primitive(PrimitiveType::Unit),
            TypeInfo::Primitive(P::Unit)
        ) | (
            Type::Primitive(PrimitiveType::Never),
            TypeInfo::Primitive(P::Never)
        ) | (
            Type::Primitive(PrimitiveType::Bool),
            TypeInfo::Primitive(P::Bool)
        ) | (
            Type::Primitive(PrimitiveType::I8),
            TypeInfo::Primitive(P::I8)
        ) | (
            Type::Primitive(PrimitiveType::I16),
            TypeInfo::Primitive(P::I16)
        ) | (
            Type::Primitive(PrimitiveType::I32),
            TypeInfo::Primitive(P::I32)
        ) | (
            Type::Primitive(PrimitiveType::I64),
            TypeInfo::Primitive(P::I64)
        ) | (
            Type::Primitive(PrimitiveType::I128),
            TypeInfo::Primitive(P::I128)
        ) | (
            Type::Primitive(PrimitiveType::ISize),
            TypeInfo::Primitive(P::Isize)
        ) | (
            Type::Primitive(PrimitiveType::U8),
            TypeInfo::Primitive(P::U8)
        ) | (
            Type::Primitive(PrimitiveType::U16),
            TypeInfo::Primitive(P::U16)
        ) | (
            Type::Primitive(PrimitiveType::U32),
            TypeInfo::Primitive(P::U32)
        ) | (
            Type::Primitive(PrimitiveType::U64),
            TypeInfo::Primitive(P::U64)
        ) | (
            Type::Primitive(PrimitiveType::U128),
            TypeInfo::Primitive(P::U128)
        ) | (
            Type::Primitive(PrimitiveType::USize),
            TypeInfo::Primitive(P::Usize)
        ) | (
            Type::Primitive(PrimitiveType::F32),
            TypeInfo::Primitive(P::F32)
        ) | (
            Type::Primitive(PrimitiveType::F64),
            TypeInfo::Primitive(P::F64)
        ) | (
            Type::Primitive(PrimitiveType::Char),
            TypeInfo::Primitive(P::Char)
        ) | (
            Type::Primitive(PrimitiveType::String),
            TypeInfo::Primitive(P::String)
        ) | (
            Type::Primitive(PrimitiveType::Bytes),
            TypeInfo::Primitive(P::Bytes)
        )
    )
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
        assert!(checked_selector_type_matches(
            &store,
            &graph,
            map,
            TypeInfoRef(3)
        ));
        assert!(!checked_selector_type_matches(
            &store,
            &graph,
            mismatched,
            TypeInfoRef(3)
        ));
        assert!(!checked_selector_type_matches(
            &store,
            &graph,
            list,
            TypeInfoRef(3)
        ));
    }
}
