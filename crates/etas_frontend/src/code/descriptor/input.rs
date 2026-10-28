use crate::{CheckedProgramData, ResolvedImports};

/// Inputs already produced by the signature and registry stages. This view is
/// not a checked-program permit; it can only project descriptive identities.
#[derive(Clone, Copy, etas_core::RetainedSize)]
pub(crate) struct DescriptorInput<'a> {
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeFacts,
    pub type_store: &'a etas_types::TypeStore,
    pub std_registry: &'a etas_std::StdRegistry,
    pub resolved_imports: &'a ResolvedImports,
    pub module_index: &'a crate::ModuleIndex,
    pub effect_registry: &'a etas_effects::EffectRegistry,
    pub effects: Option<&'a etas_effects::EffectFacts>,
}

impl<'a> From<&'a CheckedProgramData> for DescriptorInput<'a> {
    fn from(checked: &'a CheckedProgramData) -> Self {
        Self {
            hir: &checked.hir,
            types: &checked.types,
            type_store: &checked.type_store,
            std_registry: &checked.std_registry,
            resolved_imports: &checked.resolved_imports,
            module_index: &checked.module_index,
            effect_registry: &checked.effect_registry,
            effects: Some(&checked.effects),
        }
    }
}

impl std::ops::Deref for DescriptorInput<'_> {
    type Target = etas_hir::HirProgram;
    fn deref(&self) -> &Self::Target {
        self.hir
    }
}
