use std::collections::{BTreeMap, BTreeSet, HashMap};

use etas_hir::{HirExprId, HirItemId};
use etas_utils::{GraphView, strongly_connected_components};

use super::NativeInvocationTemplate;

struct Calls(BTreeMap<HirItemId, BTreeSet<HirItemId>>);

impl GraphView for Calls {
    type Node = HirItemId;

    fn nodes(&self) -> Vec<Self::Node> {
        self.0.keys().copied().collect()
    }

    fn successors(&self, owner: &Self::Node) -> Vec<Self::Node> {
        self.0.get(owner).into_iter().flatten().copied().collect()
    }
}

pub(super) fn templates(
    calls: &BTreeMap<HirItemId, Vec<HirExprId>>,
    effects: &etas_effects::EffectFacts,
) -> Vec<NativeInvocationTemplate> {
    let mut edges = BTreeMap::<_, BTreeSet<_>>::new();
    let mut sites = Vec::new();
    for (owner, calls) in calls {
        edges.entry(*owner).or_default();
        for call in calls {
            for target in effects.native_call_targets.get(call).into_iter().flatten() {
                let etas_effects::EffectUnit::Item(target) = target else {
                    continue;
                };
                edges.entry(*owner).or_default().insert(*target);
                edges.entry(*target).or_default();
                sites.push(NativeInvocationTemplate {
                    owner: *owner,
                    call: *call,
                    target: *target,
                });
            }
        }
    }
    let components = strongly_connected_components(&Calls(edges))
        .into_iter()
        .enumerate()
        .flat_map(|(component, nodes)| nodes.nodes.into_iter().map(move |node| (node, component)))
        .collect::<HashMap<_, _>>();
    sites.retain(|site| components.get(&site.owner) == components.get(&site.target));
    sites
}
