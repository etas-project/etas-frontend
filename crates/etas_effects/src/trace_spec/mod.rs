mod analysis;
mod domain;
mod lower;
mod materialize;
pub(crate) use materialize::checked_callable_owner_kind;
mod model;
mod monitor;
mod residual_eligibility;
mod semantics;
mod source_obligation;
mod source_sites;
mod static_dispositions;
mod validate;

pub use analysis::{
    CheckedPermissionCoverage, CheckedTraceClauseDisposition, CheckedTraceClauseFact,
    CheckedTracePermissionDisposition, CheckedTracePermissionFact, CheckedTraceSourceConformance,
    CheckedTraceStaticDisposition, CheckedTraceUncertainty, CheckedUncoveredAction,
    TraceSpecAnalysisInput, TraceSpecAnalysisOutput, analyze_trace_spec_monitors,
    analyze_trace_spec_monitors_controlled,
};
pub use lower::{
    AnalyzedTraceLoweringInput, LoweredTraceRules, TracePatternProjection,
    TracePatternTemplateProjection, TraceRulesLoweringError, lower_analyzed_symbolic_trace_rules,
    lower_analyzed_trace_rules, lower_symbolic_trace_rules, lower_trace_rules,
};
pub use materialize::materialize_trace_spec_models;
pub use model::{
    CheckedTraceSourceOwner, ExternalTraceSpecItem, TraceSpecArgBound, TraceSpecArgBoundTarget,
    TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecConformances, TraceSpecModelStore,
    TraceSpecPattern,
};
pub use residual_eligibility::{
    TraceResidualCause, TraceResidualClause, TraceResidualEligibility, TraceResidualIneligibility,
    TraceResidualRequirement, classify_trace_residual,
};
pub use source_obligation::{
    CheckedTraceCallableScope, TraceCallableScopeError, TraceCallableScopeInput,
    TraceObligationIdentity, TraceObligationIdentityError, TraceObligationIdentityInput,
    TraceObligationReferenceError, TraceObligationSourceError, TraceObligationSourceInput,
    TraceObligationSourceLimits, TracePlanSourceBinding, TracePlanSourceError,
    TracePlanSourceInput, TracePlanSourceLimits, checked_trace_callable_scope,
    trace_obligation_identity, validate_trace_obligation_source, validate_trace_plan_sources,
};
pub use static_dispositions::{
    CheckedTraceStaticAlternative, CheckedTraceStaticConformance, CheckedTraceStaticPermission,
    CheckedTraceStaticUncoveredAction, TraceStaticDispositions, collect_trace_static_dispositions,
};
pub use validate::{TraceSpecValidationInput, validate_trace_spec_facts};
