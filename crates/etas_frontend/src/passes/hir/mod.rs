mod common;
mod finalize;
mod lower_module_part;
mod predeclare;

pub use finalize::FinalizeProjectHirPass;
pub use lower_module_part::{LowerModuleItemsPass, NormalizeModuleImportsPass};
pub use predeclare::PredeclareProjectSymbolsPass;
