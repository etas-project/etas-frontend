use std::collections::BTreeMap;

use etas_effects::{TaskCallFact, TaskCallKind};
use etas_hir::ScopeId;

pub(super) fn task_scopes<'a>(
    calls: impl Iterator<Item = &'a TaskCallFact>,
) -> BTreeMap<ScopeId, Vec<&'a TaskCallFact>> {
    let mut owners = BTreeMap::<_, Vec<_>>::new();
    for call in calls {
        owners.entry(call.owner_scope).or_default().push(call);
    }
    for calls in owners.values_mut() {
        calls.sort_by_key(|call| call.call);
    }
    owners
}

pub(super) fn task_branches<'a>(calls: &[&'a TaskCallFact]) -> Vec<&'a TaskCallFact> {
    calls
        .iter()
        .copied()
        .filter(|call| call.kind == TaskCallKind::Start)
        .collect()
}
