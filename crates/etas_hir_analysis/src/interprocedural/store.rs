use std::collections::BTreeMap;

use etas_utils::JoinSemiLattice;

use super::AnalysisUnit;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryStore<U, S>
where
    U: AnalysisUnit,
{
    summaries: BTreeMap<U, S>,
}

impl<U, S> Default for SummaryStore<U, S>
where
    U: AnalysisUnit,
{
    fn default() -> Self {
        Self {
            summaries: BTreeMap::new(),
        }
    }
}

impl<U, S> SummaryStore<U, S>
where
    U: AnalysisUnit,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, unit: U, summary: S) -> Option<S> {
        self.summaries.insert(unit, summary)
    }

    pub fn get(&self, unit: U) -> Option<&S> {
        self.summaries.get(&unit)
    }

    pub fn contains(&self, unit: U) -> bool {
        self.summaries.contains_key(&unit)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&U, &S)> {
        self.summaries.iter()
    }
}

impl<U, S> SummaryStore<U, S>
where
    U: AnalysisUnit,
    S: JoinSemiLattice,
{
    pub fn ensure_bottom(&mut self, unit: U) {
        self.summaries.entry(unit).or_insert_with(S::bottom);
    }

    pub fn join_summary(&mut self, unit: U, summary: S) -> bool {
        match self.summaries.get_mut(&unit) {
            Some(current) => current.join_assign(&summary),
            None => {
                self.summaries.insert(unit, summary);
                true
            }
        }
    }
}
