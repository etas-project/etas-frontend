pub(crate) mod analysis;
mod default_behavior;
mod default_handlers;
pub use default_behavior::{DefaultBehavior, EffectFootprint};
pub mod domain;
pub use default_handlers::{DefaultHandlerContribution, DefaultHandlerEffects};
mod outcome;
mod requests;
pub use outcome::{
    CallableAnalysisKind, CallableExitSummary, CallableOutcome, CallableReferenceTarget,
    CallableReturnReference, CallableReturnSource, ExitTraceFact, FrozenSourceLocation,
    HandlerGapEvidence, MetadataCallableSource, MetadataObjectSource, OutcomeAnalysisGap,
    ParameterArgumentValue, ParameterExitArgument, ParameterExitFact, ParameterHandlerScopeRef,
    ReturnObservation,
};
pub use requests::{ActionRequestInstance, ParameterRequest, RequestInstantiationSummary};
pub(crate) mod memory_provenance;
pub(crate) mod semantics;
pub mod unit;

pub use domain::{
    ActionEvent, ActionEventSource, ActionTraceDomain, Determinism, EffectSummary,
    FrontendRejectionReason, HostRequirementKind, HostRequirementSet, InterpreterFeatureKind,
    InterpreterOrchestrationRequirementSet, InterpreterSupport, ParameterCallTarget,
    ResidualCheckSet, RuntimeRequirementReason,
};
pub use unit::EffectUnit;
