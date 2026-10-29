use crate::{
    AnchoredExternalMetadata, EffectRegistry, ExternalEffectSummaryMetadata,
    ToolProviderBindingMetadata,
};

#[derive(Clone)]
pub struct EffectAnalysisInput<'a> {
    pub external_implementations:
        &'a [AnchoredExternalMetadata<crate::ExternalSpecImplementationMetadata>],
    pub method_evidence: std::sync::Arc<
        std::collections::BTreeMap<
            etas_code::CodeFingerprint,
            std::sync::Arc<crate::facts::MetadataSpecMethodDefinition>,
        >,
    >,
    pub frozen_graphs: std::sync::Arc<
        std::collections::BTreeMap<
            etas_code::CodeFingerprint,
            std::sync::Arc<crate::FrozenCallableGraph>,
        >,
    >,
    pub callable_environments: std::sync::Arc<
        std::collections::BTreeMap<
            crate::facts::CallableEnvironmentId,
            crate::facts::CallableEnvironment,
        >,
    >,
    pub(crate) global_values: std::sync::Arc<super::super::semantics::state::LatentEnv>,
    pub(crate) call_results: std::sync::Arc<super::super::semantics::state::LatentEnv>,
    pub(crate) captured_values: std::sync::Arc<
        std::collections::HashMap<crate::EffectUnit, super::super::semantics::state::LatentEnv>,
    >,
    pub frozen_calls: std::sync::Arc<
        std::collections::HashMap<etas_hir::HirItemId, crate::FrozenCallableSummary>,
    >,
    pub parameter_effects:
        std::sync::Arc<std::collections::HashMap<etas_hir::SymbolId, crate::EffectSummary>>,
    pub frozen_parameter_effects:
        std::sync::Arc<std::collections::HashMap<etas_hir::SymbolId, crate::EffectSummary>>,
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeOutput,
    pub std_registry: &'a etas_std::StdRegistry,
    pub(crate) memory_provenance: std::sync::Arc<crate::infer::memory_provenance::MemoryProvenance>,
    pub registry: &'a EffectRegistry,
    pub tool_bindings: &'a [ToolProviderBindingMetadata],
    pub external_summaries: &'a [AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
}
