use etas_utils::UnitKey;

#[derive(Clone, Debug, Default)]
pub struct AffectedModuleSet {
    pub modules: Vec<UnitKey>,
    pub module_parts: Vec<UnitKey>,
    pub items: Vec<UnitKey>,
    pub bodies: Vec<UnitKey>,
}
