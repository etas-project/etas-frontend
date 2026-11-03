use crate::{
    AnchoredExternalMetadata, EffectRegistry, ExternalEffectSummaryMetadata,
    ToolProviderBindingMetadata,
};

#[derive(Default)]
pub(crate) struct EffectSourceSnapshots<'a> {
    sources: std::collections::HashMap<etas_core::SourceId, crate::EffectSourceSnapshot<'a>>,
    checked: std::sync::Mutex<
        std::collections::HashMap<
            etas_core::SourceId,
            etas_code::residual::identity::CheckedSourceSnapshot<'a>,
        >,
    >,
}

impl<'a> EffectSourceSnapshots<'a> {
    pub(crate) fn new(
        sources: &[crate::EffectSourceSnapshot<'a>],
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<Self, crate::EffectPipelineError> {
        crate::pipeline::validate_source_snapshot_identities(sources, control)?;
        Ok(Self {
            sources: sources
                .iter()
                .map(|source| (source.source, *source))
                .collect(),
            checked: Default::default(),
        })
    }

    pub(crate) fn get(
        &self,
        id: etas_core::SourceId,
    ) -> Result<Option<etas_code::residual::identity::CheckedSourceSnapshot<'a>>, ()> {
        let Some(source) = self.sources.get(&id) else {
            return Ok(None);
        };
        let mut checked = self.checked.lock().map_err(|_| ())?;
        Ok(Some(*checked.entry(id).or_insert_with(|| {
            etas_code::residual::identity::CheckedSourceSnapshot::new(
                source.key,
                source.text.as_bytes(),
            )
        })))
    }
}

#[derive(Clone)]
pub struct EffectAnalysisInput<'a> {
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
    pub residual_identity: Option<&'a dyn crate::ResidualIdentityProvider>,
    pub(crate) source_snapshots: std::sync::Arc<EffectSourceSnapshots<'a>>,
    pub types: &'a etas_types::TypeOutput,
    pub std_registry: &'a etas_std::StdRegistry,
    pub(crate) memory_provenance: std::sync::Arc<crate::infer::memory_provenance::MemoryProvenance>,
    pub registry: &'a EffectRegistry,
    pub tool_bindings: &'a [ToolProviderBindingMetadata],
    pub external_summaries: &'a [AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
}
