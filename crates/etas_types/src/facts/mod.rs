pub mod effects;
pub mod enums;
pub mod expressions;
pub mod inference;
pub mod method_specs;
pub mod reflection;
pub mod signatures;
pub mod specs;
pub mod symbols;

use std::collections::HashMap;

use etas_hir::{HirExprId, HirItemId, HirPatId, HirStmtId, HirTypeId, SymbolId};

pub use effects::{
    ActionRequestSignature, CheckedActionCallFact, CheckedHandlerActionFact,
    DeferredHandlerInstantiation, DeferredHandlerOverload, EffectActionArgKind,
    EffectActionSignature, HandlerTypeBinding,
};
pub use enums::{EnumLayoutFact, EnumVariantLayoutFact};
pub use expressions::{
    CheckedIndexKind, CheckedSliceKind, CheckedStdCallFact, DeferredEffectRowObligation,
    FieldProjectionFact, GenericInstantiationFact, TryExprTypeFact,
};
pub use inference::CheckedAgentInferenceFact;
pub use reflection::{CheckedReflectionFact, CheckedReflectionTarget};
pub use signatures::{
    AgentEntrySignature, AgentSignature, CallableGenericParam, CallableGenericParamKind,
    CallableSignature, FlowSignature, ItemSignature, ToolSignature, TopLevelLetSignature,
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

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeFacts {
    pub external_specs: specs::ExternalSpecFacts,
    pub external_type_aliases: HashMap<Vec<String>, (Vec<String>, TypeId)>,
    pub type_generics: HashMap<TypeId, Vec<CallableGenericParam>>,
    pub reflection_type_identities: HashMap<TypeId, etas_code::DefinitionKey>,
    pub reflection_declaration_identities: HashMap<Vec<String>, etas_code::DefinitionKey>,
    pub reflection_effect_targets: HashMap<String, etas_code::descriptor::EffectTargetInfo>,
    pub external_definition_bindings: HashMap<SymbolId, etas_code::DefinitionKey>,
    pub external_agent_reflections: HashMap<SymbolId, reflection::ExternalAgentReflectionFact>,
    pub agent_methods: std::collections::HashMap<
        etas_hir::SymbolId,
        crate::facts::signatures::AgentMethodSignature,
    >,
    pub agent_method_calls: HashMap<HirExprId, SymbolId>,
    pub std_intrinsic_calls: HashMap<HirExprId, CheckedStdCallFact>,
    pub action_calls: HashMap<HirExprId, CheckedActionCallFact>,
    pub expr_types: HashMap<HirExprId, TypeId>,
    pub reflections: HashMap<HirExprId, CheckedReflectionFact>,
    pub definition_operations: HashMap<HirExprId, reflection::CheckedDefinitionOperation>,
    pub agent_inferences: HashMap<HirExprId, CheckedAgentInferenceFact>,
    pub handler_actions: HashMap<etas_hir::HirHandlerArmId, CheckedHandlerActionFact>,
    pub deferred_handler_instantiations:
        HashMap<etas_hir::HirHandlerArmId, DeferredHandlerInstantiation>,
    pub expr_memory_places: HashMap<HirExprId, TypeId>,
    #[serde(default)]
    pub field_projections: HashMap<HirExprId, Vec<FieldProjectionFact>>,
    pub stmt_types: HashMap<HirStmtId, TypeId>,
    pub pattern_types: HashMap<HirPatId, TypeId>,
    pub type_refs: HashMap<HirTypeId, TypeId>,
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    pub agent_components: HashMap<TypeId, SymbolId>,
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

impl TypeFacts {
    pub fn enum_constructor<'a>(
        &'a self,
        store: &crate::TypeStore,
        symbol: &etas_hir::Symbol,
    ) -> Option<(TypeId, usize, &'a EnumVariantLayoutFact)> {
        let (owner, index) =
            enums::constructor_identity(store, symbol, self.symbol_types.get(&symbol.id)?)?;
        let variant = self.enum_layouts.get(&owner)?.variants.get(index)?;
        (variant.name == symbol.name).then_some((owner, index, variant))
    }

    pub fn agent_default_method(
        &self,
        agent: HirItemId,
    ) -> Option<&signatures::AgentMethodSignature> {
        let ItemSignature::Agent(signature) = self.item_signatures.get(&agent)? else {
            return None;
        };
        let method = self
            .agent_methods
            .get(&signature.default_entry.as_ref()?.method?)?;
        (method.agent == agent).then_some(method)
    }

    pub fn agent_signature(&self, component: TypeId) -> Option<&AgentSignature> {
        let symbol = self.agent_components.get(&component)?;
        match self.symbol_types.get(symbol)? {
            SymbolTypeFact::Agent { signature } if signature.component == component => {
                Some(signature)
            }
            _ => None,
        }
    }
}

impl From<&TypeFacts> for SpecFacts {
    fn from(facts: &TypeFacts) -> Self {
        Self {
            external_signatures: facts.external_specs.signatures.clone(),
            external_impls: facts.external_specs.impls.clone(),
            external_aliases: facts.external_specs.aliases.clone(),
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
