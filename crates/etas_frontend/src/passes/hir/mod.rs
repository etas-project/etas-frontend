mod common;
mod execution_role;
mod finalize;
mod lower_module_part;
pub(crate) mod native_role;
mod predeclare;
mod source_prelude;
mod source_types;

pub use finalize::FinalizeProjectHirPass;
pub use lower_module_part::{LowerModuleItemsPass, NormalizeModuleImportsPass};
pub use predeclare::PredeclareProjectSymbolsPass;
