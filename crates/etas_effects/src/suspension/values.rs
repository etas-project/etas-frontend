use std::collections::BTreeSet;

use super::{CallContext, HirExprId, Instance, Node, SuspensionSummary, instances};
use crate::facts::CallableValueOrigin;

pub(super) fn call(
    context: CallContext<'_>,
    call: HirExprId,
    sources: &BTreeSet<CallableValueOrigin>,
    node: &mut Node,
) {
    let sources = instances::resolve(context, sources);
    if sources.is_empty() {
        node.local.join(SuspensionSummary::UNKNOWN);
    }
    for source in sources {
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
        if let Some(instance) =
            instance.filter(|instance| context.effects.unit_effects.contains_key(&instance.unit))
        {
            node.calls.insert(instance);
        } else {
            node.local.join(SuspensionSummary::UNKNOWN);
        }
    }
}
