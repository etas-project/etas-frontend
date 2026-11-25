use std::collections::BTreeSet;

use super::{CallContext, HirExprId, Instance, Node, SuspensionSummary, instances};
use crate::facts::CallableValueOrigin;

pub(super) fn call(
    context: CallContext<'_>,
    call: HirExprId,
    sources: &BTreeSet<CallableValueOrigin>,
    node: &mut Node,
) {
    if sources.is_empty() {
        node.local.join(SuspensionSummary::UNKNOWN);
    }
    let mut pending = sources
        .iter()
        .copied()
        .map(|source| (source, None))
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some((source, inputs)) = pending.pop() {
        let inputs: Option<(u32, std::sync::Arc<crate::facts::NativeCallInputs>)> = inputs;
        if !visited.insert((source, inputs.as_ref().map(|(id, _)| *id))) {
            if matches!(
                source,
                CallableValueOrigin::Parameter { .. } | CallableValueOrigin::Projection(_)
            ) {
                node.local.join(SuspensionSummary::UNKNOWN);
            }
            continue;
        }
        // A checked call boundary may already bind this exact invocation.
        // Use that evidence before expanding its generic parameter recipe.
        let resolved = instances::resolve(context, &BTreeSet::from([source]));
        if resolved != BTreeSet::from([source]) {
            if resolved.is_empty() {
                node.local.join(SuspensionSummary::UNKNOWN);
            }
            pending.extend(resolved.into_iter().map(|source| (source, inputs.clone())));
            continue;
        }
        if let Some((sources, id, recipe)) = instances::application(context, source) {
            pending.extend(
                sources
                    .into_iter()
                    .map(|source| (source, Some((id, recipe.clone())))),
            );
            continue;
        }
        if let CallableValueOrigin::Metadata(source) = source {
            // A frozen transformer describes a public contract, not an edited
            // body's execution. Keep the same unknown result as frozen units.
            if source.analysis != crate::CallableAnalysisKind::CheckedRevision
                || context
                    .effects
                    .frozen_callable_graphs
                    .contains_key(&source.graph)
            {
                node.local.join(SuspensionSummary::UNKNOWN);
                continue;
            }
            let summary = context
                .effects
                .metadata_callable_graphs
                .get(&source.graph)
                .and_then(|graph| graph.graph.values.get(source.value as usize))
                .and_then(|value| value.behavior.suspension)
                .map(|summary| SuspensionSummary {
                    may_suspend: summary.may_suspend,
                    unknown: summary.unknown,
                })
                .unwrap_or(SuspensionSummary::UNKNOWN);
            node.local.join(summary);
            continue;
        }
        let captured = match source {
            CallableValueOrigin::Captured { environment, .. } => Some(environment),
            CallableValueOrigin::Reference { environment, .. } => environment,
            _ => None,
        };
        let instance = match source {
            CallableValueOrigin::Unit(unit) => instances::source(context, call, unit),
            CallableValueOrigin::Captured { unit, environment } => {
                let mut instance = Instance::declaration(unit);
                instance.bindings = context.bindings.to_vec();
                instance.values = context.values.clone();
                instances::captured(context, instance, environment)
                    .map(|instance| instances::arguments(context, call, instance))
            }
            CallableValueOrigin::Reference {
                unit,
                value,
                environment,
            } => instances::source(context, value, unit).and_then(|instance| {
                let instance = if let Some(environment) = environment {
                    instances::captured(context, instance, environment)?
                } else {
                    instance
                };
                Some(instances::arguments(context, call, instance))
            }),
            // A frozen contract or an unresolved/imported callback needs its
            // own proven execution summary. An empty Effect row is insufficient.
            _ => None,
        };
        let instance = match inputs {
            Some((_, inputs)) => instance.and_then(|instance| {
                instances::application_arguments(context, &inputs, instance, captured)
            }),
            None => instance,
        };
        if let Some(instance) =
            instance.filter(|instance| context.effects.unit_effects.contains_key(&instance.unit))
        {
            node.calls.insert(instance);
        } else {
            node.local.join(SuspensionSummary::UNKNOWN);
        }
    }
}
