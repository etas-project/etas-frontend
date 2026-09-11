use std::collections::HashMap;

use etas_hir::{HirItemId, SymbolId};

use crate::{
    CallableSpecSatisfactionFact, EffectActionSignature, ItemSignature, ResourceHandleFact,
    SpecImplFact, SpecSignature, SymbolTypeFact, TraceSpecConformanceFact, TypeId,
    TypeParamBoundFact, TypeSpecSatisfactionFact,
};

#[derive(Default)]
pub struct SignaturePipelineState {
    pub enum_layouts: HashMap<TypeId, crate::EnumLayoutFact>,
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    pub resource_handles: HashMap<SymbolId, ResourceHandleFact>,
    pub item_signatures: HashMap<HirItemId, ItemSignature>,
    pub action_signatures: HashMap<SymbolId, EffectActionSignature>,
    pub qualified_action_signatures: HashMap<String, EffectActionSignature>,
    pub spec_signatures: HashMap<SymbolId, SpecSignature>,
    pub spec_impls: Vec<SpecImplFact>,
    pub type_spec_satisfactions: Vec<TypeSpecSatisfactionFact>,
    pub callable_spec_satisfactions: Vec<CallableSpecSatisfactionFact>,
    pub trace_spec_conformances: Vec<TraceSpecConformanceFact>,
    pub type_param_bounds: HashMap<SymbolId, Vec<TypeParamBoundFact>>,
    pub aliases: HashMap<SymbolId, TypeId>,
}
