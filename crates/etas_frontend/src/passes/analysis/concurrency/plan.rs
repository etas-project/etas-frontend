use std::collections::HashMap;

use etas_effects::{
    EffectFacts, EffectRegistry, InterpreterSupport, LimitKind, LimitValue, RequirementFact,
    TaskCallFact, TaskCallKind,
};

use super::{
    BranchFact, ConcurrencyFacts, ConcurrencySiteFact, SchedulingDecision, TaskScopeFact, conflict,
    ownership,
};

pub(super) fn analyze(effects: &EffectFacts, registry: &EffectRegistry) -> ConcurrencyFacts {
    let owners = ownership::task_scopes(effects.task_calls.values());
    let mut scope_facts = HashMap::new();
    let mut sites = HashMap::new();

    for (owner_scope, calls) in owners {
        let task_calls = ownership::task_branches(&calls);
        let branches = task_calls
            .iter()
            .map(|call| BranchFact {
                task_site: call.call,
                effects: call.effects.clone(),
                actions: call.behavior.action_trace.action_set(),
            })
            .collect::<Vec<_>>();
        let incomplete = task_calls.iter().any(|call| {
            call.effects.open.is_some()
                || call.behavior.requested_actions.open.is_some()
                || call.behavior.action_trace.has_parameter_calls()
        });
        let rejected = calls
            .iter()
            .any(|call| matches!(call.behavior.support, InterpreterSupport::Rejected(_)));
        let decision = if rejected {
            SchedulingDecision::Reject
        } else {
            conflict::scheduling_decision(&branches, registry, incomplete)
        };
        let concurrency_limit = calls
            .iter()
            .filter_map(|call| concurrency_limit(call))
            .min();
        let scope = TaskScopeFact {
            owner_scope,
            sites: calls.iter().map(|call| call.call).collect(),
            branches: branches.clone(),
            decision: decision.clone(),
            concurrency_limit,
        };
        for call in calls {
            let scheduling = match call.kind {
                TaskCallKind::Await | TaskCallKind::Requeue | TaskCallKind::Park => {
                    SchedulingDecision::Concurrent
                }
                _ => decision.clone(),
            };
            sites.insert(
                call.call,
                ConcurrencySiteFact {
                    kind: call.kind,
                    decision: scheduling,
                    branches: branches.clone(),
                    concurrency_limit,
                },
            );
        }
        scope_facts.insert(owner_scope, scope);
    }

    ConcurrencyFacts {
        sites,
        owners: scope_facts,
    }
}

fn concurrency_limit(call: &TaskCallFact) -> Option<u32> {
    call.behavior.requirements.iter().find_map(|requirement| {
        let RequirementFact::Limit(limit) = requirement else {
            return None;
        };
        if limit.kind != LimitKind::Concurrency {
            return None;
        }
        let Some(LimitValue::Count(value)) = limit.value else {
            return None;
        };
        u32::try_from(value).ok()
    })
}
