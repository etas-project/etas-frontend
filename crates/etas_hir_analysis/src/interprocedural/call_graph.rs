use std::collections::{BTreeMap, BTreeSet};

use etas_utils::GraphView;

use super::AnalysisUnit;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallGraph<U>
where
    U: AnalysisUnit,
{
    dependencies: BTreeMap<U, BTreeSet<U>>,
    dependents: BTreeMap<U, BTreeSet<U>>,
}

impl<U> Default for CallGraph<U>
where
    U: AnalysisUnit,
{
    fn default() -> Self {
        Self {
            dependencies: BTreeMap::new(),
            dependents: BTreeMap::new(),
        }
    }
}

impl<U> CallGraph<U>
where
    U: AnalysisUnit,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_unit(&mut self, unit: U) {
        self.dependencies.entry(unit).or_default();
        self.dependents.entry(unit).or_default();
    }

    pub fn add_call(&mut self, caller: U, callee: U) {
        self.add_unit(caller);
        self.add_unit(callee);
        self.dependencies.entry(caller).or_default().insert(callee);
        self.dependents.entry(callee).or_default().insert(caller);
    }

    pub fn units(&self) -> Vec<U> {
        self.dependencies.keys().copied().collect()
    }

    pub fn dependencies(&self, unit: U) -> Vec<U> {
        self.dependencies
            .get(&unit)
            .map(|units| units.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn dependents(&self, unit: U) -> Vec<U> {
        self.dependents
            .get(&unit)
            .map(|units| units.iter().copied().collect())
            .unwrap_or_default()
    }
}

impl<U> GraphView for CallGraph<U>
where
    U: AnalysisUnit + Eq,
{
    type Node = U;

    fn nodes(&self) -> Vec<Self::Node> {
        self.units()
    }

    fn successors(&self, node: &Self::Node) -> Vec<Self::Node> {
        self.dependencies(*node)
    }
}
