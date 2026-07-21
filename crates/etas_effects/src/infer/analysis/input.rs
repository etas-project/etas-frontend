use crate::{
    AnchoredExternalMetadata, EffectRegistry, ExternalEffectSummaryMetadata,
    ToolProviderBindingMetadata,
};

#[derive(Clone)]
pub struct EffectAnalysisInput<'a> {
    pub hir: &'a etas_hir::HirProgram,
    pub types: &'a etas_types::TypeOutput,
    pub registry: EffectRegistry,
    pub tool_bindings: &'a [ToolProviderBindingMetadata],
    pub external_summaries: &'a [AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
}
