mod common;
mod finalize;
mod lower_module_part;
pub(crate) mod native_binding;
mod predeclare;

pub use finalize::FinalizeProjectHirPass;
pub use lower_module_part::{LowerModuleItemsPass, NormalizeModuleImportsPass};
pub use predeclare::PredeclareProjectSymbolsPass;
