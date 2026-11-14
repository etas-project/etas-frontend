pub mod concurrency;
pub mod effects;
pub mod enums;
pub mod existentials;
pub mod expressions;
pub mod inference;
pub mod method_specs;
pub mod methods;
pub mod reflection;
pub mod signatures;
mod spec_method;
mod spec_signature;
pub mod specs;
pub mod symbols;

use std::collections::HashMap;

use etas_hir::{HirExprId, HirItemId, HirPatId, HirStmtId, HirTypeId, SymbolId};

pub use concurrency::{ContinuationBindingFact, TaskOwnerFact};
pub use effects::{
    ActionRequestSignature, CheckedActionCallFact, CheckedHandlerActionFact,
    DeferredHandlerInstantiation, DeferredHandlerOverload, EffectActionArgKind,
    EffectActionSignature, HandlerTypeBinding,
};
pub use enums::{EnumLayoutFact, EnumVariantLayoutFact};
mod impl_methods;
pub use expressions::{
    CheckedIndexKind, CheckedSliceKind, CheckedStdCallFact, DeferredEffectRowObligation,
    EffectRowEquation, FieldProjectionFact, GenericInstantiationFact, TryExprTypeFact,
};
pub use impl_methods::TypeImplMethodFact;
pub use inference::CheckedAgentInferenceFact;
pub use methods::{BoundMethodReceiver, CheckedBoundMethodValueFact, CheckedMethodCallFact};
pub use reflection::{CheckedReflectionFact, CheckedReflectionTarget};
pub use signatures::{
    AgentEntrySignature, AgentSignature, CallableGenericParam, CallableGenericParamKind,
    CallableSignature, FlowSignature, ItemSignature, ToolSignature, TopLevelLetSignature,
};
pub use specs::{
    CallableSpecSatisfactionFact, CanonicalSpecFacts, CanonicalSpecImplFact,
    CanonicalSpecSignatureFact, CheckedSpecBound, CheckedSpecRef, CheckedStdSpecImplFact,
    ExternalCallableSpecSatisfactionFact, ExternalSpecIdentity, ExternalTraceSpecConformanceFact,
    ExternalTraceSpecConformanceTarget, RelocatedSpecFacts, SpecFacts, SpecImplFact,
    SpecImplMethodFact, SpecKind, SpecMethodFact, SpecMethodIdentity, SpecSignature,
    SpecSuperBoundFact, TraceSpecConformanceFact, TraceSpecConformanceTarget, TypeParamBoundFact,
    TypeSpecSatisfactionFact,
};
pub use symbols::{KnownStdTypes, ResourceHandleFact, SymbolTypeFact};

use crate::TypeId;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct TypeFacts {
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub existential_packs: HashMap<HirExprId, existentials::CheckedExistentialPackFact>,
    #[serde(default)]
    pub methods: std::sync::Arc<crate::method::MethodIndex>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub checked_method_calls: HashMap<HirExprId, CheckedMethodCallFact>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub bound_method_values: HashMap<HirExprId, methods::CheckedBoundMethodValueFact>,
    #[serde(default)]
    pub canonical_specs: CanonicalSpecFacts,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub type_impl_methods: HashMap<SymbolId, TypeImplMethodFact>,
    pub external_specs: specs::ExternalSpecFacts,
    pub external_methods: Vec<methods::ExternalMethodFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub external_type_aliases: HashMap<Vec<String>, (Vec<String>, TypeId)>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub external_type_declarations: HashMap<Vec<String>, TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub source_type_roles: HashMap<SymbolId, etas_std::StdSymbolId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub type_generics: HashMap<TypeId, Vec<CallableGenericParam>>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub reflection_type_identities: HashMap<TypeId, etas_code::DefinitionKey>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub reflection_declaration_identities: HashMap<Vec<String>, etas_code::DefinitionKey>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub reflection_effect_targets: HashMap<String, etas_code::descriptor::EffectTargetInfo>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub external_definition_bindings: HashMap<SymbolId, etas_code::DefinitionKey>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub external_agent_reflections: HashMap<SymbolId, reflection::ExternalAgentReflectionFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub agent_methods: std::collections::HashMap<
        etas_hir::SymbolId,
        crate::facts::signatures::AgentMethodSignature,
    >,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub agent_method_calls: HashMap<HirExprId, SymbolId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub std_intrinsic_calls: HashMap<HirExprId, CheckedStdCallFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub action_calls: HashMap<HirExprId, CheckedActionCallFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub expr_types: HashMap<HirExprId, TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub reflections: HashMap<HirExprId, CheckedReflectionFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub definition_operations: HashMap<HirExprId, reflection::CheckedDefinitionOperation>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub agent_inferences: HashMap<HirExprId, CheckedAgentInferenceFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub handler_actions: HashMap<etas_hir::HirHandlerArmId, CheckedHandlerActionFact>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub continuation_bindings: HashMap<SymbolId, ContinuationBindingFact>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub task_owners: HashMap<HirExprId, TaskOwnerFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub deferred_handler_instantiations:
        HashMap<etas_hir::HirHandlerArmId, DeferredHandlerInstantiation>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub expr_memory_places: HashMap<HirExprId, TypeId>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub field_projections: HashMap<HirExprId, Vec<FieldProjectionFact>>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub stmt_types: HashMap<HirStmtId, TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub pattern_types: HashMap<HirPatId, TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub type_refs: HashMap<HirTypeId, TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub symbol_types: HashMap<SymbolId, SymbolTypeFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub agent_components: HashMap<TypeId, SymbolId>,
    pub known_std_types: KnownStdTypes,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub enum_layouts: HashMap<TypeId, EnumLayoutFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub resource_handles: HashMap<SymbolId, ResourceHandleFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub item_signatures: HashMap<HirItemId, ItemSignature>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub std_callable_signatures: HashMap<etas_std::StdSymbolId, signatures::CallableSignature>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub action_signatures: HashMap<SymbolId, EffectActionSignature>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub qualified_action_signatures: HashMap<String, EffectActionSignature>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub spec_signatures: HashMap<SymbolId, SpecSignature>,
    pub spec_impls: Vec<SpecImplFact>,
    pub type_spec_satisfactions: Vec<TypeSpecSatisfactionFact>,
    pub callable_spec_satisfactions: Vec<CallableSpecSatisfactionFact>,
    pub trace_spec_conformances: Vec<TraceSpecConformanceFact>,
    pub external_callable_spec_satisfactions: Vec<ExternalCallableSpecSatisfactionFact>,
    pub external_trace_spec_conformances: Vec<ExternalTraceSpecConformanceFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub type_param_bounds: HashMap<SymbolId, Vec<TypeParamBoundFact>>,
    pub std_spec_impls: Vec<CheckedStdSpecImplFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub std_spec_aliases: HashMap<SymbolId, Vec<String>>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub index_facts: HashMap<HirExprId, CheckedIndexKind>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub slice_facts: HashMap<HirExprId, CheckedSliceKind>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub checked_index_errors: HashMap<HirExprId, TypeId>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub generic_instantiations: HashMap<HirExprId, GenericInstantiationFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub try_facts: HashMap<HirExprId, TryExprTypeFact>,
}

impl TypeFacts {
    pub fn selected_method(&self, expr: etas_hir::HirExprId) -> Option<&CheckedMethodCallFact> {
        self.checked_method_calls.get(&expr).or_else(|| {
            self.bound_method_values
                .get(&expr)
                .map(|value| &value.method)
        })
    }
    /// The producer ABI before an implicit, checked existential pack.
    pub fn expression_producer_type(&self, expression: HirExprId) -> Option<TypeId> {
        self.existential_packs
            .get(&expression)
            .map(|pack| pack.source_type)
            .or_else(|| self.expr_types.get(&expression).copied())
    }

    pub fn enum_constructor<'a>(
        &'a self,
        store: &crate::TypeStore,
        symbol: &etas_hir::Symbol,
    ) -> Option<(TypeId, usize, &'a EnumVariantLayoutFact)> {
        let (owner, index) =
            enums::constructor_identity(store, symbol, self.symbol_types.get(&symbol.id)?)?;
        let variant = self.enum_layouts.get(&owner)?.variants.get(index)?;
        (variant.name == enums::constructor_name(symbol)).then_some((owner, index, variant))
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
            canonical: facts.canonical_specs.clone(),
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
