use super::AnalysisUnit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisPhase {
    DiscoverCalls,
    SolveSummaries,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitContext<U>
where
    U: AnalysisUnit,
{
    pub unit: U,
    pub phase: AnalysisPhase,
}

impl<U> UnitContext<U>
where
    U: AnalysisUnit,
{
    pub fn new(unit: U, phase: AnalysisPhase) -> Self {
        Self { unit, phase }
    }
}
