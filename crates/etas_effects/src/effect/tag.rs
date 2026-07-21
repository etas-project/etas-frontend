use super::EffectTagId;
use crate::RuntimeRequirementReason;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectTag {
    pub id: EffectTagId,
    pub name: String,
    pub core: Option<CoreEffect>,
    pub runtime_requirement: Option<RuntimeRequirementReason>,
    pub high_impact_ack: bool,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum CoreEffect {
    Agentic,
    Network,
    FileIO,
    Command,
    Memory,
    Secret,
    Time,
    Human,
    Error,
}
