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
    InferCallableOutputsPass, MaterializeGenericEffectRowsPass, PrepareBodyTypeEnvironmentPass,
    PrepareInvocationTypesPass, RetainDefinitionSourcesPass, ReuseTypeBodyFactsPass,
    RunEffectPipelinePass, TypeCheckBodyPass, ValidateExternalEnvironmentPass,
    ValidateHandlerInstantiationsPass, ValidateItemAnnotationsPass, ValidateTopLevelLetPass,
};
pub use dependency::{
    CompleteEffectRuntimeReachabilityPass, CompleteTypedAnalysisReachabilityPass,
    CompleteTypedRuntimeReachabilityPass, ComputeDefinitionAnalysisReachabilityPass,
    ComputeEntryReachabilityPass,
};
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
    ApplyResolvedImportsToHirPass, BindValidatedExternalImportTargetsPass, CatalogModuleProvider,
    ModuleResolution, ProjectModuleResolver, ProviderExport, ProviderExportTarget,
    ResolveImportTargetsPass, ResolvePathsPass,
};
pub use source::{BuildSourceSetPass, ParseSourceFilePass};
