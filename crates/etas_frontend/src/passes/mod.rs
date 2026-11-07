mod analysis;
pub(crate) mod artifacts;
mod common;
mod dependency;
pub(crate) mod execution;
mod graph;
mod hir;
mod module;
mod optimization;
mod output;
mod resolution;
mod source;

pub(crate) use analysis::CheckedStandardLibraryImplementations;
pub use analysis::concurrency::{
    AwaitSiteId, BranchFact, ConcurrencyFacts, ConcurrencyPlanFact, ConcurrencySiteFact,
    ContinuationTransferFact, ContinuationTransferId, ResourceConflict, ResourceGuardCall,
    ResourceGuardCallablePlan, ResourceGuardPlan, ResourceValueSelector, RuntimeResourceSelector,
    SchedulingDecision, TaskAwaitFact, TaskBranchFact, TaskBranchShape, TaskScopeFact, TaskSiteId,
    TaskStartFact,
};
pub use analysis::{
    AnalyzeConcurrencyPass, AnalyzeCpuParallelEligibilityPass, AnalyzeLoopProgressPass,
    BindMethodOriginsPass, BuildSignatureFactsPass, CheckStandardLibraryImplementationsPass,
    CpuCallableOrigin, CpuCaptureContract, CpuFreezeContract, CpuParallelEligibilityFact,
    CpuParallelEligibilityFacts, CpuParallelRejection, FinalizeTypeFactsPass,
    InferCallableOutputsPass, MaterializeGenericEffectRowsPass, PrepareBodyTypeEnvironmentPass,
    PrepareDirectInvocationTypesPass, PrepareEffectRegistryPass, PrepareInvocationTypesPass,
    RetainDefinitionSourcesPass, ReuseTypeBodyFactsPass, RunEffectPipelinePass, TypeCheckBodyPass,
    ValidateExternalEnvironmentPass, ValidateHandlerInstantiationsPass,
    ValidateItemAnnotationsPass, ValidateTopLevelLetPass,
};
pub use dependency::{
    CompleteEffectRuntimeReachabilityPass, CompleteTypedAnalysisReachabilityPass,
    CompleteTypedRuntimeReachabilityPass, ComputeDefinitionAnalysisReachabilityPass,
    ComputeEntryReachabilityPass,
};
pub use execution::{ValidateResidualBackendSupportPass, VerifyInterpreterSupportPass};
pub use graph::{
    BuildImportGraphPass, ComputeAffectedModulesPass, ComputeModuleTopoOrderPass,
    DetectImportCyclesPass,
};
pub use hir::{
    FinalizeProjectHirPass, LowerModuleItemsPass, NormalizeModuleImportsPass,
    PredeclareProjectSymbolsPass,
};
pub use module::{BuildModuleCatalogPass, BuildModuleIndexPass, BuildUnitTreePass};
pub use optimization::{
    AgentEffectLegalityPass, ApplyQualityContextBudgetPolicyPass, BatchStorageSemijoinPass,
    FuseProjectionScansPass, FuseSerializationBoundaryPass, LowerOwnedSnapshotCowPass,
    LowerResourceRegionPass, PrepareVersionedRequestPass, ProjectAgentObservationPass,
    PruneSupersededAgentEventsPass, PruneUnusedQualityContextPass, ScheduleBoundedPipelinePass,
    ShortCircuitAgentThresholdPass, SpecializeAgentToolSchemasPass,
    SpecializeDeterministicDispatchPass, SuffixDemandHistoryPass, SynthesizeScopedIndexPass,
    VersionedPreparationCsePass,
};
pub use output::{BuildCheckedProjectPass, ResolveEntryItemPass, ValidateEntryContractPass};
pub use resolution::{
    ApplyResolvedImportsToHirPass, BindValidatedExternalImportTargetsPass, CatalogModuleProvider,
    ModuleResolution, ProjectModuleResolver, ProviderExport, ProviderExportTarget,
    ResolveImportTargetsPass, ResolvePathsPass,
};
pub(crate) use source::AdmittedStandardLibrarySource;
pub use source::{
    AdmitStandardLibrarySourcesPass, AdmittedStandardLibrarySources, BuildSourceSetPass,
    ParseSourceFilePass,
};
