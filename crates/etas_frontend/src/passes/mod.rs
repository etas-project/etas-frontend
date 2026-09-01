mod analysis;
pub(crate) mod artifacts;
mod common;
mod dependency;
mod execution;
mod graph;
mod hir;
mod module;
mod output;
mod resolution;
mod source;

pub use analysis::{
    AnalyzeLoopProgressPass, BuildSignatureFactsPass, FinalizeTypeFactsPass,
    ReuseTypeBodyFactsPass, RunEffectPipelinePass, TypeCheckBodyPass,
    ValidateExternalEnvironmentPass, ValidateTopLevelLetPass,
};
pub use dependency::ComputeEntryReachabilityPass;
pub use execution::VerifyInterpreterSupportPass;
pub use graph::{
    BuildImportGraphPass, ComputeAffectedModulesPass, ComputeModuleTopoOrderPass,
    DetectImportCyclesPass,
};
pub use hir::{
    FinalizeProjectHirPass, LowerModuleItemsPass, NormalizeModuleImportsPass,
    PredeclareProjectSymbolsPass,
};
pub use module::{BuildModuleCatalogPass, BuildModuleIndexPass, BuildUnitTreePass};
pub use output::{BuildCheckedProjectPass, ResolveEntryItemPass, ValidateEntryContractPass};
pub use resolution::{
    ApplyResolvedImportsToHirPass, CatalogModuleProvider, ModuleResolution, ProjectModuleResolver,
    ProviderExport, ProviderExportTarget, ResolveImportTargetsPass, ResolvePathsPass,
};
pub use source::{BuildSourceSetPass, ParseSourceFilePass};
