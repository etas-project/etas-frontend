mod diagnostic_anchor;
pub mod effect;
mod external_trace;
pub mod facts;
pub mod infer;
pub mod pipeline;
mod pipeline_error;
pub mod requirement;
pub mod residual;
mod residual_identity;
pub mod substitution;
pub mod trace_spec;
pub mod validate;

pub use effect::{
    AGENTIC_INFER_ACTION, AGENTIC_TAG, APPROVAL_REQUEST_ACTION, APPROVAL_TAG, ActionInstanceRef,
    ActionRef, COMMAND_RUN_ACTION, COMMAND_TAG, CONSOLE_STDERR_WRITE_ACTION,
    CONSOLE_STDIN_READ_ALL_ACTION, CONSOLE_STDIN_READ_LINE_ACTION, CONSOLE_STDOUT_WRITE_ACTION,
    CONSOLE_TAG, CoreEffect, DependencyEffectAction, DependencyEffectExtension,
    DependencyEffectMetadata, DependencyEffectTag, DependencySourceEffectBinding, ERROR_TAG,
    Effect, EffectActionArgKind, EffectActionId, EffectActionSig, EffectCoverage, EffectRegistry,
    EffectRow, EffectSet, EffectTag, EffectTagId, EffectVarId, ExtensionGraph, FILE_IO_TAG,
    HUMAN_TAG, MEMORY_READ_ACTION, MEMORY_TAG, MEMORY_WRITE_ACTION, MemoryPlaceDecl, NETWORK_TAG,
    SECRET_TAG, TIME_TAG, ToolProviderBindingMetadata, UnresolvedDependencyEffectExtension,
    effect_var_id_from_name,
};
pub use facts::{
    BodyEffectOutput, ContinuationTransferFact, EffectAcknowledgement, EffectAnalysisOutput,
    EffectComponentConvergence, EffectConvergenceStatus, EffectFacts, EffectMaterializationInputs,
    EffectOutput, ErrorConversionFact, FinishSummary, HandleApplicationFact, HandlerActionFact,
    HandlerArmFact, HandlerCompletionSummary, HandlerValueFact, HandlerValueRef,
    InterpreterSupportFacts, LatentEffectBodyRef, LatentEffectContract, LatentEffectFact,
    PerformedActionFact, PublicEffectContract, PublicEffectContractSource, PublicEffectValue,
    QualifiedName, RequirementFacts, RequirementFactsForContract, ResumeSummary,
    StdCallableEffectFact, TaskCallFact, TaskCallKind, TraceSpecClauseFact, TraceSpecFacts,
    TransferTarget, TryCaptureFact,
};
pub use infer::{
    ActionEvent, ActionEventSource, ActionRequestInstance, ActionTraceDomain, CallableAnalysisKind,
    CallableExitSummary, CallableOutcome, CallableReferenceTarget, CallableReturnReference,
    CallableReturnSource, DefaultBehavior, DefaultHandlerContribution, DefaultHandlerEffects,
    Determinism, EffectFootprint, EffectSummary, EffectUnit, ExitTraceFact,
    FrontendRejectionReason, FrozenSourceLocation, HandlerGapEvidence, HostRequirementKind,
    HostRequirementSet, InterpreterFeatureKind, InterpreterOrchestrationRequirementSet,
    InterpreterSupport, MetadataCallableSource, MetadataObjectSource, OutcomeAnalysisGap,
    ParameterArgumentValue, ParameterCallTarget, ParameterExitArgument, ParameterExitFact,
    ParameterHandlerScopeRef, ParameterRequest, RequestInstantiationSummary, ResidualCheckSet,
    ReturnObservation, RuntimeRequirementReason,
};
pub use pipeline::{
    AnchoredExternalMetadata, EffectInferencePlan, EffectPipelineArtifacts, EffectPipelineInput,
    EffectPipelineOutput, EffectSourceSnapshot, ExternalActionRequestMetadata,
    ExternalActionTraceMetadata, ExternalArtifactAnchor, ExternalCallableContract,
    ExternalCallableRequestTarget, ExternalCallableValueMetadata, ExternalDefaultBehaviorMetadata,
    ExternalDefaultHandlerContributionMetadata, ExternalDefaultHandlerEffectsMetadata,
    ExternalEffectFootprintMetadata, ExternalEffectMetadata, ExternalEffectRowMetadata,
    ExternalEffectSummaryMetadata, ExternalExistentialMemberMetadata,
    ExternalExistentialValueMetadata, ExternalLatentFlowSummaryMetadata,
    ExternalMethodRequirements, ExternalMethodTraceRequirement, ExternalParameterRequestMetadata,
    ExternalRequestInstantiationMetadata, ExternalSpecImplementationMetadata,
    ExternalSpecMethodCallableArgument, ExternalSpecMethodGenericArgument,
    ExternalSpecMethodInvocation, ExternalTraceSpecClauseKind, ExternalTraceSpecClauseMetadata,
    ExternalTraceSpecEffectMetadata, ExternalTraceSpecEffectRowMetadata,
    ExternalTraceSpecSummaryMetadata, PreparedEffectRegistry, RunEffectPipeline,
    TraceSpecPipelineAnalysis,
};
pub use pipeline_error::{EffectPipelineError, EffectStage};
pub use requirement::{
    LimitBudgetKind, LimitKind, LimitRequirement, LimitValue, RequirementFact, RequirementSet,
};
pub use residual_identity::{
    ResidualIdentityError, ResidualIdentityProvider, TraceMonitorProjectionRequest,
    TraceMonitorProjector, TraceMonitorSourceLimits,
};
mod frozen;
pub use frozen::{
    FrozenCallableEnvironment, FrozenCallableGraph, FrozenCallableGraphIdentity,
    FrozenCallableSummary, FrozenCallableSummaryProvider,
};
