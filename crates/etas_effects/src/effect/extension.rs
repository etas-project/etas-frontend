use std::collections::{BTreeMap, BTreeSet};

use super::EffectTagId;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ExtensionGraph {
    parents: BTreeMap<EffectTagId, BTreeSet<EffectTagId>>,
}

impl ExtensionGraph {
    pub fn add_extension(&mut self, child: EffectTagId, parent: EffectTagId) {
        self.parents.entry(child).or_default().insert(parent);
    }

    pub fn directly_extends(&self, child: EffectTagId, parent: EffectTagId) -> bool {
        self.parents
            .get(&child)
            .is_some_and(|parents| parents.contains(&parent))
    }

    pub fn extends(&self, child: EffectTagId, ancestor: EffectTagId) -> bool {
        if child == ancestor {
            return true;
        }
        let mut stack = vec![child];
        let mut seen = BTreeSet::new();
        while let Some(tag) = stack.pop() {
            if !seen.insert(tag) {
                continue;
            }
            let Some(parents) = self.parents.get(&tag) else {
                continue;
            };
            if parents.contains(&ancestor) {
                return true;
            }
            stack.extend(parents.iter().copied());
        }
        false
    }
}
