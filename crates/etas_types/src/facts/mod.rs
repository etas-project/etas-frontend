pub mod effects;
pub mod expressions;
pub mod signatures;
pub mod specs;
pub mod symbols;

use std::collections::HashMap;

use etas_hir::{HirExprId, HirItemId, HirPatId, HirStmtId, HirTypeId, SymbolId};

pub use effects::{EffectActionArgKind, EffectActionSignature};
pub use expressions::{
    CheckedIndexKind, CheckedSliceKind, GenericInstantiationFact, TryExprTypeFact,
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
    pub stmt_types: HashMap<HirStmtId, TypeId>,
    pub pattern_types: HashMap<HirPatId, TypeId>,
    pub type_refs: HashMap<HirTypeId, TypeId>,
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    pub known_std_types: KnownStdTypes,
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
