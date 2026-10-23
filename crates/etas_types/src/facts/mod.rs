pub mod effects;
pub mod enums;
pub mod expressions;
pub mod signatures;
pub mod specs;
pub mod symbols;

use std::collections::HashMap;

use etas_hir::{HirExprId, HirItemId, HirPatId, HirStmtId, HirTypeId, SymbolId};

pub use effects::{EffectActionArgKind, EffectActionSignature};
pub use enums::{EnumLayoutFact, EnumVariantLayoutFact};
pub use expressions::{
    CheckedIndexKind, CheckedSliceKind, DeferredEffectRowObligation, FieldProjectionFact,
    GenericInstantiationFact, TryExprTypeFact,
};
pub use signatures::{
    AgentSignature, CallableGenericParam, CallableGenericParamKind, CallableSignature,
    FlowSignature, ItemSignature, ToolSignature, TopLevelLetSignature,
};
pub use specs::{
    CallableSpecSatisfactionFact, CheckedSpecBound, CheckedSpecRef, CheckedStdSpecImplFact,
    ExternalCallableSpecSatisfactionFact, ExternalTraceSpecConformanceFact,
    ExternalTraceSpecConformanceTarget, SpecFacts, SpecImplFact, SpecImplMethodFact, SpecKind,
    SpecMethodFact, SpecMethodIdentity, SpecSignature, SpecSuperBoundFact,
    TraceSpecConformanceFact, TraceSpecConformanceTarget, TypeParamBoundFact,
    TypeSpecSatisfactionFact,
};
pub use symbols::{KnownStdTypes, ResourceHandleFact, SymbolTypeFact};

use crate::TypeId;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TypeFacts {
    pub expr_types: HashMap<HirExprId, TypeId>,
    pub expr_memory_places: HashMap<HirExprId, TypeId>,
    #[serde(default)]
    pub field_projections: HashMap<HirExprId, Vec<FieldProjectionFact>>,
    pub stmt_types: HashMap<HirStmtId, TypeId>,
    pub pattern_types: HashMap<HirPatId, TypeId>,
    pub type_refs: HashMap<HirTypeId, TypeId>,
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    pub known_std_types: KnownStdTypes,
    #[serde(default)]
    pub enum_layouts: HashMap<TypeId, EnumLayoutFact>,
    pub resource_handles: HashMap<SymbolId, ResourceHandleFact>,
    pub item_signatures: HashMap<HirItemId, ItemSignature>,
    pub action_signatures: HashMap<SymbolId, EffectActionSignature>,
    #[serde(default)]
    pub qualified_action_signatures: HashMap<String, EffectActionSignature>,
    pub spec_signatures: HashMap<SymbolId, SpecSignature>,
    pub spec_impls: Vec<SpecImplFact>,
    pub type_spec_satisfactions: Vec<TypeSpecSatisfactionFact>,
    pub callable_spec_satisfactions: Vec<CallableSpecSatisfactionFact>,
    pub trace_spec_conformances: Vec<TraceSpecConformanceFact>,
    pub external_callable_spec_satisfactions: Vec<ExternalCallableSpecSatisfactionFact>,
    pub external_trace_spec_conformances: Vec<ExternalTraceSpecConformanceFact>,
    pub type_param_bounds: HashMap<SymbolId, Vec<TypeParamBoundFact>>,
    pub std_spec_impls: Vec<CheckedStdSpecImplFact>,
    pub std_spec_aliases: HashMap<SymbolId, Vec<String>>,
    pub index_facts: HashMap<HirExprId, CheckedIndexKind>,
    pub slice_facts: HashMap<HirExprId, CheckedSliceKind>,
    pub checked_index_errors: HashMap<HirExprId, TypeId>,
    pub generic_instantiations: HashMap<HirExprId, GenericInstantiationFact>,
    pub try_facts: HashMap<HirExprId, TryExprTypeFact>,
}

impl From<&TypeFacts> for SpecFacts {
    fn from(facts: &TypeFacts) -> Self {
        Self {
            signatures: facts.spec_signatures.clone(),
            impls: facts.spec_impls.clone(),
            type_satisfactions: facts.type_spec_satisfactions.clone(),
            callable_satisfactions: facts.callable_spec_satisfactions.clone(),
            trace_conformances: facts.trace_spec_conformances.clone(),
            external_callable_satisfactions: facts.external_callable_spec_satisfactions.clone(),
            external_trace_conformances: facts.external_trace_spec_conformances.clone(),
            type_param_bounds: facts.type_param_bounds.clone(),
            std_impls: facts.std_spec_impls.clone(),
            std_spec_aliases: facts.std_spec_aliases.clone(),
        }
    }
}
