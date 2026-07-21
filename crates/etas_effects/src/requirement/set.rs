use std::collections::BTreeSet;

use super::LimitRequirement;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum RequirementFact {
    Limit(LimitRequirement),
    TraceSpec(crate::TraceSpecClauseFact),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequirementSet {
    facts: BTreeSet<RequirementFact>,
}

impl RequirementSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn one(fact: RequirementFact) -> Self {
        Self::from_iter([fact])
    }

    pub fn insert(&mut self, fact: RequirementFact) -> bool {
        self.facts.insert(fact)
    }

    pub fn contains(&self, fact: &RequirementFact) -> bool {
        self.facts.contains(fact)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RequirementFact> {
        self.facts.iter()
    }

    pub fn union_assign(&mut self, other: &RequirementSet) -> bool {
        let len = self.facts.len();
        self.facts.extend(other.facts.iter().cloned());
        self.facts.len() != len
    }
}

impl FromIterator<RequirementFact> for RequirementSet {
    fn from_iter<T: IntoIterator<Item = RequirementFact>>(iter: T) -> Self {
        Self {
            facts: iter.into_iter().collect(),
        }
    }
}
