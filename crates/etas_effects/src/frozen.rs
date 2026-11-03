use crate::{EffectPipelineError, EffectRegistry, EffectSummary};
use etas_hir::{HirItemId, SymbolId};
use std::collections::HashMap;

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub enum FrozenCallableSummary {
    Solved(std::sync::Arc<EffectSummary>),
    /// The compiler owns the unchanged body in its pinned source archive, but
    /// the original entry-reachable check did not analyze this declaration.
    AnalyzePinnedSource,
    Unavailable,
}

impl FrozenCallableSummary {
    pub(crate) fn uses_snapshot(&self) -> bool {
        !matches!(self, Self::AnalyzePinnedSource)
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct FrozenCallableEnvironment {
    pub method_evidence: std::collections::BTreeMap<
        etas_code::CodeFingerprint,
        std::sync::Arc<crate::facts::MetadataSpecMethodDefinition>,
    >,
    pub graphs:
        std::collections::BTreeMap<etas_code::CodeFingerprint, std::sync::Arc<FrozenCallableGraph>>,
    pub callable_environments: std::collections::BTreeMap<
        crate::facts::CallableEnvironmentId,
        crate::facts::CallableEnvironment,
    >,
    pub callables: HashMap<HirItemId, FrozenCallableSummary>,
    pub parameter_effects: HashMap<SymbolId, EffectSummary>,
}

/// Compiler-owned, session-local behavior proof. It is not a package import or
/// executable body and carries no producer authority.
#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct FrozenCallableGraph {
    pub identity: FrozenCallableGraphIdentity,
    pub graph: std::sync::Arc<etas_package_metadata::CallableValueGraph>,
    pub values: Vec<crate::ExternalCallableValueMetadata>,
    pub exits: Vec<Option<crate::CallableExitSummary>>,
    pub anchor: etas_core::Span,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct FrozenCallableGraphIdentity {
    pub environment: etas_code::CodeFingerprint,
    pub callable: etas_code::CallableId,
    pub graph: etas_code::CodeFingerprint,
    pub owner: HirItemId,
}

impl FrozenCallableGraphIdentity {
    pub fn key(&self) -> etas_code::CodeFingerprint {
        let callable = self.callable.fingerprint();
        etas_code::CodeFingerprint::of_parts(
            "etas.effects.frozen-callable-graph.v1",
            [
                self.environment.as_bytes().as_slice(),
                callable.as_bytes(),
                self.graph.as_bytes(),
            ],
        )
    }
}

/// Native declaration bindings supplied by a compiler-owned frozen environment,
/// not by portable metadata or caller-authored descriptor graphs.
pub trait FrozenCallableSummaryProvider: etas_core::memory::RetainedSize {
    fn project(
        &self,
        registry: &EffectRegistry,
    ) -> Result<FrozenCallableEnvironment, EffectPipelineError>;
}
