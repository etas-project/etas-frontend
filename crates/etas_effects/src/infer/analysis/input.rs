use crate::{
    AnchoredExternalMetadata, EffectRegistry, ExternalEffectSummaryMetadata,
    ToolProviderBindingMetadata,
};

#[derive(Clone)]
pub struct EffectAnalysisInput<'a> {
    pub hir: &'a etas_hir::HirProgram,
    pub types: &'a etas_types::TypeOutput,
    pub std_registry: &'a etas_std::StdRegistry,
    pub registry: &'a EffectRegistry,
    pub tool_bindings: &'a [ToolProviderBindingMetadata],
    pub external_summaries: &'a [AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
}
