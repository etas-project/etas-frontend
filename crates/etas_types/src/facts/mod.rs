pub mod concurrency;
pub(crate) mod dependencies;
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
    /// Direct type dependencies emitted with each solved body, before packaging.
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub body_spec_dependencies: HashMap<HirItemId, std::collections::BTreeSet<SymbolId>>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub body_type_dependencies: HashMap<HirItemId, std::collections::BTreeSet<TypeId>>,

    /// Calls to the installed Trusted constructor whose payload is a source string literal.
    #[serde(default)]
    pub trusted_literals: std::collections::BTreeSet<HirExprId>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub declared_call_arguments: HashMap<HirExprId, crate::ArgumentBinding>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub source_execution_roles: HashMap<SymbolId, etas_std::StdExecutionRole>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub external_execution_roles: HashMap<etas_code::MethodId, etas_std::StdExecutionRole>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub scoped_callbacks: HashMap<SymbolId, concurrency::ScopedCallbackFact>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub external_scoped_methods: HashMap<etas_code::MethodId, Vec<u32>>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub external_resource_boundaries: HashMap<etas_code::MethodId, Vec<u32>>,
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
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub source_flow_calls: HashMap<HirExprId, SymbolId>,
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub library_flow_calls: HashMap<HirExprId, expressions::CheckedLibraryCallFact>,
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
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub agent_controllers: HashMap<HirExprId, inference::CheckedAgentControllerFact>,
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
    #[serde(default, serialize_with = "etas_utils::serde_ordered::map")]
    pub library_callable_signatures: HashMap<etas_code::CallableId, signatures::CallableSignature>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub action_signatures: HashMap<SymbolId, EffectActionSignature>,
    #[serde(default)]
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub qualified_action_signatures: HashMap<String, EffectActionSignature>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    pub spec_signatures: HashMap<SymbolId, SpecSignature>,
    pub spec_impls: Vec<SpecImplFact>,
    #[serde(default)]
    pub checked_spec_impls: Vec<specs::CheckedSpecImplementationFact>,
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
    /// The closed carrier traversed by the checked model response boundary.
    pub fn model_step_provider_map(&self, store: &crate::TypeStore) -> Option<TypeId> {
        let is_step_result = |ty| match store.get(ty) {
            Some(crate::Type::Applied { constructor, .. }) => matches!(
                store.get(TypeId(constructor.0)),
                Some(crate::Type::Nominal(nominal)) if nominal.name == "std.agent.StepResult"
            ),
            _ => false,
        };
        if self.agent_controllers.is_empty()
            && !self
                .action_calls
                .values()
                .any(|call| is_step_result(call.signature.output))
            && !self
                .library_flow_calls
                .values()
                .any(|call| is_step_result(call.signature.output))
            && !self
                .std_intrinsic_calls
                .values()
                .any(|call| is_step_result(call.signature.output))
        {
            return None;
        }
        let fields = store.iter().find_map(|(_, ty)| match ty {
            crate::Type::Nominal(nominal) if nominal.name == "std.agent.AssistantTurn" => {
                match store.get(nominal.representation?)? {
                    crate::Type::Record(record) => Some(record),
                    _ => None,
                }
            }
            _ => None,
        })?;
        fields
            .fields
            .iter()
            .find(|field| field.name == "provider_data")
            .map(|field| field.ty)
    }

    /// Checked source adapters retain private collection algorithms across
    /// the SDK boundary. A same-named application extension is not a contract.
    pub fn storage_methods(
        &self,
    ) -> impl Iterator<Item = (&crate::method::MethodSignatureFact, &etas_code::MethodId)> {
        [
            etas_std::SOURCE_STORAGE_VIEW,
            etas_std::SOURCE_STORAGE_BUILD,
        ]
        .into_iter()
        .flat_map(|name| self.methods.member(name))
        .filter_map(|(_, method)| {
            if method.public {
                return None;
            }
            let identity = self.methods.target_identity(&method.target)?;
            let implementation = identity.implementation();
            let owner = implementation.subject();
            (implementation.package() == owner.package()
                && implementation.module() == owner.module()
                && etas_std::source_storage_method(
                    owner.module(),
                    owner.declaration(),
                    &method.name,
                ))
            .then_some((method, identity))
        })
    }

    /// Fixed, private standard value operations. Their checked package/owner
    /// identities prevent an application extension from replacing language ==.
    pub fn value_equality_methods(
        &self,
    ) -> impl Iterator<Item = (&crate::method::MethodSignatureFact, &etas_code::MethodId)> {
        self.methods
            .member(etas_std::SOURCE_VALUE_EQUALITY_METHOD)
            .filter_map(|(_, method)| {
                if method.public || method.name != etas_std::SOURCE_VALUE_EQUALITY_METHOD {
                    return None;
                }
                let identity = self.methods.target_identity(&method.target)?;
                let implementation = identity.implementation();
                let owner = implementation.subject();
                (implementation.package() == owner.package()
                    && implementation.module() == owner.module()
                    && etas_std::source_value_equality_owner(owner.module(), owner.declaration()))
                .then_some((method, identity))
            })
    }

    /// Type-to-operation edges, including operations of nested value types.
    pub fn value_equality_dependencies(
        &self,
        store: &crate::TypeStore,
    ) -> std::collections::BTreeSet<etas_code::MethodId> {
        let mut pending = self
            .expr_types
            .values()
            .copied()
            .chain(
                self.generic_instantiations
                    .values()
                    .flat_map(|fact| fact.type_bindings.iter().map(|(_, ty)| *ty)),
            )
            .collect::<Vec<_>>();
        let mut found = std::collections::BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if found.insert(ty) {
                if let Some(value) = store.get(ty) {
                    pending.extend(crate::type_dependencies(value));
                }
            }
        }
        self.value_equality_methods()
            .filter_map(|(method, identity)| {
                let owner = match store.get(method.receiver_type) {
                    Some(crate::Type::Applied { constructor, .. }) => crate::TypeId(constructor.0),
                    _ => method.receiver_type,
                };
                found.contains(&owner).then(|| identity.clone())
            })
            .collect()
    }

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
        let (owner, index) = enums::constructor_identity(
            store,
            symbol,
            self.symbol_types.get(&symbol.id)?,
            &self.known_std_types,
        )?;
        let variant = self.enum_layouts.get(&owner)?.variants.get(index)?;
        (variant.name == enums::constructor_name(symbol)).then_some((owner, index, variant))
    }

    pub fn agent_method(
        &self,
        component: TypeId,
        name: &str,
    ) -> Option<&signatures::AgentMethodSignature> {
        let candidates = || {
            self.agent_methods
                .values()
                .filter(|method| method.component == component && method.name == name)
        };
        let mut inherent = candidates().filter(|method| method.inherent);
        if let Some(method) = inherent.next() {
            return inherent.next().is_none().then_some(method);
        }
        let mut candidates = candidates();
        let method = candidates.next()?;
        candidates.next().is_none().then_some(method)
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

    pub fn is_active_agent(&self, store: &crate::TypeStore, ty: TypeId) -> bool {
        let component = match store.get(ty) {
            Some(crate::Type::Applied { constructor, .. }) => TypeId(constructor.0),
            _ => ty,
        };
        self.agent_signature(component)
            .is_some_and(|agent| agent.state.is_some())
    }

    /// The private layout is separate from the scoped reference's nominal type.
    pub fn agent_state_type(
        &self,
        store: &crate::TypeStore,
        ty: TypeId,
    ) -> Result<Option<TypeId>, crate::TypeSubstitutionError> {
        let (component, args) = match store.get(ty) {
            Some(crate::Type::Applied { constructor, args }) => (TypeId(constructor.0), Some(args)),
            _ => (ty, None),
        };
        let Some(state) = self
            .agent_signature(component)
            .and_then(|agent| agent.state)
        else {
            return Ok(None);
        };
        let Some(args) = args else {
            return Ok(Some(state));
        };
        let names = self
            .type_generics
            .get(&component)
            .into_iter()
            .flatten()
            .map(|parameter| parameter.name.clone())
            .zip(args.iter().copied())
            .collect();
        let canonical = crate::ty::canonical_parameters_for_application(store, component, args)?;
        crate::TypeSubstitutionEngine::existing(store, &names)
            .with_canonical_parameters(&canonical)
            .substitute(state)
            .map(Some)
    }
}

impl From<&TypeFacts> for SpecFacts {
    fn from(facts: &TypeFacts) -> Self {
        Self {
            active_agents: facts
                .symbol_types
                .values()
                .filter_map(|fact| match fact {
                    SymbolTypeFact::Agent { signature } => Some(signature.component),
                    _ => None,
                })
                .collect(),
            data_declarations: crate::data_admission::Declarations::from_facts(facts),
            checked_impls: facts.checked_spec_impls.clone(),
            canonical: facts.canonical_specs.clone(),
            external_signatures: facts.external_specs.signatures.clone(),
            external_impls: facts.external_specs.impls.clone(),
            external_aliases: facts.external_specs.aliases.clone(),
            external_std_specs: facts.external_specs.std_identities.clone(),
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
            canonical_std_specs: facts
                .std_spec_aliases
                .iter()
                .filter_map(|(symbol, path)| {
                    facts
                        .external_definition_bindings
                        .get(symbol)
                        .or_else(|| facts.reflection_declaration_identities.get(path))
                        .filter(|identity| identity.kind() == etas_code::DefinitionKind::Spec)
                        .map(|identity| (identity.clone(), path.clone()))
                })
                .chain(
                    facts
                        .external_specs
                        .std_identities
                        .iter()
                        .map(|(external, identity)| (identity.clone(), external.path.clone())),
                )
                .collect(),
        }
    }
}

impl TypeFacts {
    /// Concrete generic arguments bring their checked implementation witnesses
    /// across a package boundary, even when the bound call is inside a library
    /// body rather than in the consumer HIR. Follow witness owner arguments for
    /// nested containers instead of loading every implementation in the SDK.
    pub fn generic_method_dependencies(
        &self,
        store: &crate::TypeStore,
    ) -> Result<std::collections::BTreeSet<etas_code::MethodId>, String> {
        let specs = crate::SpecFacts::from(self);
        let lookup = crate::ty::TypeLookupIndex::new(store);
        let mut pending = self
            .generic_instantiations
            .values()
            .flat_map(|fact| fact.type_bindings.iter().map(|(_, ty)| *ty))
            .chain(
                self.checked_method_calls
                    .values()
                    .flat_map(|call| call.owner_bindings.iter().map(|(_, ty)| *ty)),
            )
            .collect::<Vec<_>>();
        let mut visited = std::collections::BTreeSet::new();
        let mut methods = std::collections::BTreeSet::new();
        let mut lookup_witnesses = self.methods.indexed_witness_lookup(store, &lookup, &specs);
        while let Some(receiver) = pending.pop() {
            if !visited.insert(receiver) {
                continue;
            }
            for witness in lookup_witnesses(receiver)? {
                if let crate::method::MethodTarget::External { identity } =
                    &witness.declaration.target
                {
                    methods.insert(identity.clone());
                }
                pending.extend(witness.owner_bindings.values().copied());
            }
        }
        Ok(methods)
    }

    /// External method roots selected by checked calls, values and witnesses.
    pub fn referenced_external_methods(&self) -> std::collections::BTreeSet<etas_code::MethodId> {
        let targets = self
            .checked_method_calls
            .values()
            .map(|call| &call.target)
            .chain(
                self.bound_method_values
                    .values()
                    .map(|value| &value.method.target),
            )
            .chain(
                self.existential_packs
                    .values()
                    .flat_map(|pack| &pack.witnesses)
                    .map(|witness| &witness.target),
            )
            .collect::<Vec<_>>();
        let mut methods = targets
            .iter()
            .filter_map(|target| match target {
                crate::method::MethodTarget::External { identity } => Some(identity.clone()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let members = targets
            .iter()
            .filter_map(|target| match target {
                crate::method::MethodTarget::BoundSpec { member, .. } => Some(member),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        if !members.is_empty() {
            methods.extend(self.methods.entries().filter_map(|method| {
                match (&method.target, &method.spec_member) {
                    (crate::method::MethodTarget::External { identity }, Some(member))
                        if members.contains(member) =>
                    {
                        Some(identity.clone())
                    }
                    _ => None,
                }
            }));
        }
        methods
    }
}
