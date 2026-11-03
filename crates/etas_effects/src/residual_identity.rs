use etas_code::CodeFingerprint;
use etas_hir::HirItemId;
use etas_types::TypeId;

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub enum ResidualIdentityError {
    Unavailable(String),
    ResourceLimit {
        resource: &'static str,
        limit: usize,
    },
    Interrupted(etas_utils::PipelineInterruption),
}

impl std::fmt::Display for ResidualIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => f.write_str(reason),
            Self::ResourceLimit { resource, limit } => {
                write!(f, "{resource} exceeds limit {limit}")
            }
            Self::Interrupted(interruption) => f.write_str(&interruption.reason),
        }
    }
}

/// Projects checked, closed generic arguments into portable descriptor
/// identities. These identities are not residual installation evidence.
pub trait ResidualIdentityProvider: etas_core::memory::RetainedSize {
    fn closed_type(
        &self,
        registry: &crate::EffectRegistry,
        owner: HirItemId,
        ty: TypeId,
    ) -> Result<CodeFingerprint, ResidualIdentityError>;
    fn closed_row(
        &self,
        registry: &crate::EffectRegistry,
        owner: HirItemId,
        row: &crate::EffectRow,
    ) -> Result<CodeFingerprint, ResidualIdentityError>;
}

/// Source-link limits applied by a canonical trace monitor projection.
#[derive(Clone, Copy, Debug)]
pub struct TraceMonitorSourceLimits {
    pub max_sources: usize,
    pub max_sites: usize,
    pub max_clauses: usize,
    pub max_snapshot_bytes: usize,
    pub identity: etas_code::residual::identity::ObligationIdentityLimits,
}

/// One analyzed conformance to project into the shared monitor model. The
/// projection must use exactly the model captured by `lowering.analysis`.
#[derive(Clone, Copy)]
pub struct TraceMonitorProjectionRequest<'a> {
    pub registry: &'a crate::EffectRegistry,
    pub facts: &'a crate::EffectFacts,
    pub lowering: crate::trace_spec::AnalyzedTraceLoweringInput<'a>,
    pub source_limits: TraceMonitorSourceLimits,
}

/// Checked canonical trace monitor projection. A successful projection is an
/// input to residual plan lowering; it does not cover an obligation, prove
/// backend support or install a monitor.
pub trait TraceMonitorProjector: etas_core::memory::RetainedSize {
    fn project_trace_monitor(
        &self,
        request: TraceMonitorProjectionRequest<'_>,
    ) -> Result<etas_code::residual::monitor::CompiledTraceAlternatives, ResidualIdentityError>;

    /// The contract reference for obligations created by this compilation:
    /// the checked source constraint, or an already frozen input contract.
    fn residual_contract(&self) -> etas_code::residual::identity::BehaviorContractRef {
        etas_code::residual::identity::BehaviorContractRef::SourceConstraint
    }
}
