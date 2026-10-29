use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use etas_code::{
    CallableId, CodeFingerprint, DefinitionKey,
    descriptor::{AgentHeaderInfo, CallableHeaderInfo, DescriptorGraph, DescriptorLimits},
};
use etas_hir::{HirExprId, HirItemId, HirModuleId, SymbolId};
use etas_types::{CheckedReflectionTarget, CheckedSpecRef};

use crate::{
    CheckedProgram,
    code::{
        descriptor::ProjectionError,
        identity::{DefinitionIdentityIndex, DefinitionOrigins},
    },
};

#[derive(etas_core::RetainedSize)]
pub(super) struct NativeCallableSlot {
    pub canonical: CallableId,
    pub item: HirItemId,
    pub private_to: Option<HirModuleId>,
}

#[derive(etas_core::RetainedSize)]
pub(super) struct CatalogCallable {
    pub header: CallableHeaderInfo,
    pub native: Arc<NativeCallableSlot>,
}

#[derive(etas_core::RetainedSize)]
pub(super) struct CatalogAgent {
    pub header: AgentHeaderInfo,
    pub private_to: Option<HirModuleId>,
    pub component: etas_types::TypeId,
}

pub enum StaticReflectionView<'a> {
    Agent(AgentHeaderInfo),
    Callable(&'a CallableHeaderInfo),
}

#[derive(etas_core::RetainedSize)]
pub struct DefinitionCatalog {
    pub(super) checked: CheckedProgram,
    pub(super) origins: Arc<DefinitionOrigins>,
    pub(super) identities: DefinitionIdentityIndex,
    pub(super) graph: DescriptorGraph,
    pub(super) type_origins: Vec<crate::code::descriptor::NativeTypeOrigin>,
    pub(super) callables: BTreeMap<CallableId, CatalogCallable>,
    pub(super) external_callables: BTreeMap<CallableId, super::external::ExternalCallable>,
    pub(super) external_agents: BTreeMap<DefinitionKey, super::external::ExternalAgent>,
    pub(super) agents: BTreeMap<DefinitionKey, CatalogAgent>,
    pub(super) callable_bindings: HashMap<SymbolId, CallableId>,
    pub(super) agent_bindings: HashMap<SymbolId, DefinitionKey>,
    pub(super) selected_bindings: HashMap<(SymbolId, DefinitionKey), CallableId>,
}

impl DefinitionCatalog {
    pub(crate) fn exported_descriptors(
        checked: CheckedProgram,
        origins: &DefinitionOrigins,
        identities: DefinitionIdentityIndex,
        selection: &std::collections::BTreeSet<HirItemId>,
        limits: DescriptorLimits,
    ) -> Result<etas_code::descriptor::DefinitionDescriptorSet, ProjectionError> {
        super::collect::build_exported(checked, origins, identities, limits, selection)
    }
    pub fn checked_program(&self) -> &crate::CheckedProgram {
        &self.checked
    }
    pub fn freeze_agent(
        &self,
        expression: HirExprId,
    ) -> Result<Arc<super::FrozenDefinitionEnvironment>, ProjectionError> {
        if !self.checked.definition_sources.revisions().is_empty() {
            return Err(ProjectionError::RuntimeRevisionRequired);
        }
        let StaticReflectionView::Agent(agent) = self.static_reflection(expression)? else {
            return Err(ProjectionError::Invalid(
                "agent compilation requires AgentDef<A>, not CallableDef".into(),
            ));
        };
        let fact = self
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(ProjectionError::MissingReflection(expression))?;
        let CheckedReflectionTarget::Agent { declaration, .. } = fact.target else {
            return Err(ProjectionError::RuntimeRevisionRequired);
        };
        let item = match self
            .checked
            .symbols
            .get(declaration)
            .map(|symbol| &symbol.def)
        {
            Some(etas_hir::SymbolDef::Item { item }) => *item,
            _ => return Err(ProjectionError::MissingCallable(declaration)),
        };
        self.freeze_agent_view(agent, item)
    }

    pub fn freeze_flow(
        &self,
        expression: HirExprId,
    ) -> Result<Arc<super::FrozenDefinitionEnvironment>, ProjectionError> {
        let (header, item) = self.editable_flow_reflection(expression)?;
        self.freeze_flow_view(header, item)
    }

    pub fn freeze_callable_application(
        &self,
        application: &crate::CheckedCallableApplication,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<Arc<super::FrozenDefinitionEnvironment>, ProjectionError> {
        if !self.checked.definition_sources.revisions().is_empty() {
            return Err(ProjectionError::RuntimeRevisionRequired);
        }
        let contract =
            super::FrozenDefinitionContract::capture_application(self, application, limits)?;
        self.freeze_contract(contract, application.owner())
    }

    pub(super) fn editable_flow_reflection(
        &self,
        expression: HirExprId,
    ) -> Result<(&CallableHeaderInfo, HirItemId), ProjectionError> {
        let fact = self
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(ProjectionError::MissingReflection(expression))?;
        let typed = matches!(self.checked.type_store.get(fact.descriptor_type),
            Some(etas_types::Type::Applied { constructor, args })
                if Some(etas_types::TypeId(constructor.0)) == self.checked.types.known_std_types.code.callable_definition && args.len() == 1);
        if !typed {
            return Err(ProjectionError::Invalid(
                "editing requires a typed ordinary-flow definition, not an erased method view"
                    .into(),
            ));
        }
        let StaticReflectionView::Callable(header) = self.static_reflection(expression)? else {
            return Err(ProjectionError::Invalid(
                "flow compilation requires CallableDef<F>".into(),
            ));
        };
        let item = self.item_owner(&header.id).ok_or_else(|| {
            ProjectionError::MissingIdentity("frozen flow source declaration".into())
        })?;
        Ok((header, item))
    }

    pub(super) fn freeze_flow_view(
        &self,
        header: &CallableHeaderInfo,
        item: HirItemId,
    ) -> Result<Arc<super::FrozenDefinitionEnvironment>, ProjectionError> {
        if !self.checked.definition_sources.revisions().is_empty() {
            return Err(ProjectionError::RuntimeRevisionRequired);
        }
        let Some(etas_hir::HirItem::Flow(flow)) = self.checked.items.get(item) else {
            return Err(ProjectionError::Invalid(
                "frozen ordinary-flow declaration is missing".into(),
            ));
        };
        if self.checked.types.agent_methods.contains_key(&flow.symbol)
            || self.callable_bindings.get(&flow.symbol) != Some(&header.id)
            || self.item_owner(&header.id) != Some(item)
        {
            return Err(ProjectionError::Invalid(
                "frozen flow declaration and canonical identity disagree".into(),
            ));
        }
        let contract = super::FrozenDefinitionContract::capture_flow(self, header)?;
        self.freeze_contract(contract, item)
    }

    pub(super) fn freeze_agent_view(
        &self,
        agent: AgentHeaderInfo,
        item: HirItemId,
    ) -> Result<Arc<super::FrozenDefinitionEnvironment>, ProjectionError> {
        if !self.checked.definition_sources.revisions().is_empty() {
            return Err(ProjectionError::RuntimeRevisionRequired);
        }
        let symbol = self
            .checked
            .items
            .get(item)
            .and_then(etas_hir::HirItem::symbol)
            .ok_or_else(|| {
                ProjectionError::Invalid("frozen agent declaration is missing".into())
            })?;
        if self.agent_bindings.get(&symbol) != Some(&agent.identity) {
            return Err(ProjectionError::Invalid(
                "frozen agent declaration and canonical identity disagree".into(),
            ));
        }
        // Access filtering belongs to the view, not the immutable revision.
        let agent = self
            .agents
            .get(&agent.identity)
            .ok_or(ProjectionError::MissingDeclaration(symbol))?
            .header
            .clone();
        let contract = super::FrozenDefinitionContract::capture_agent(self, agent)?;
        self.freeze_contract(contract, item)
    }

    fn freeze_contract(
        &self,
        contract: Arc<super::FrozenDefinitionContract>,
        declaration: HirItemId,
    ) -> Result<Arc<super::FrozenDefinitionEnvironment>, ProjectionError> {
        let body = contract
            .entry()
            .map(|id| {
                self.item_owner(id).ok_or_else(|| {
                    ProjectionError::MissingIdentity("frozen default callable owner".into())
                })
            })
            .transpose()?;
        let callable_slots = contract
            .callables()
            .map(|(id, _)| {
                let item = self.item_owner(id).ok_or_else(|| {
                    ProjectionError::MissingIdentity("frozen callable native body owner".into())
                })?;
                let slot = self
                    .checked
                    .definition_sources
                    .body(item)
                    .cloned()
                    .ok_or_else(|| {
                        ProjectionError::MissingIdentity("frozen callable native body slot".into())
                    })?;
                Ok((id.clone(), slot))
            })
            .collect::<Result<_, ProjectionError>>()?;
        super::FrozenDefinitionEnvironment::new(
            self.checked.clone(),
            contract,
            declaration,
            body,
            self.origins.clone(),
            self.body_revision_fingerprint()?,
            callable_slots,
        )
    }

    fn body_revision_fingerprint(&self) -> Result<CodeFingerprint, ProjectionError> {
        let mut revisions = BTreeMap::new();
        for callable in self.callables.values() {
            let Some(revision) = self
                .checked
                .definition_sources
                .overlaid_body(callable.native.item)
            else {
                continue;
            };
            let content = revision.syntax.identity().content();
            if let Some(previous) = revisions.insert(&callable.native.canonical, content)
                && previous != content
            {
                return Err(ProjectionError::Invalid(
                    "canonical callable has conflicting body revisions".into(),
                ));
            }
        }
        if revisions.len() != self.checked.definition_sources.revisions().len() {
            return Err(ProjectionError::Invalid(
                "body revision has no unique canonical callable identity".into(),
            ));
        }
        let bindings = revisions
            .into_iter()
            .map(|(callable, content)| {
                CodeFingerprint::of_parts(
                    "etas.code.callable-body-binding.v1",
                    [
                        callable.fingerprint().as_bytes().as_slice(),
                        content.as_bytes().as_slice(),
                    ],
                )
            })
            .collect::<Vec<_>>();
        Ok(CodeFingerprint::of_parts(
            "etas.code.body-revision-bindings.v1",
            bindings.iter().map(|binding| binding.as_bytes().as_slice()),
        ))
    }

    pub fn retained_body(
        &self,
        expression: HirExprId,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<Option<etas_code::syntax::SyntaxBlockRef>, ProjectionError> {
        let callable = match self.static_reflection(expression)? {
            StaticReflectionView::Agent(agent) => agent.default_entry,
            StaticReflectionView::Callable(callable) => Some(callable.id.clone()),
        };
        let Some(callable) = callable else {
            return Ok(None);
        };
        self.retained_callable_body(expression, &callable, syntax)
    }

    pub(super) fn retained_callable_body(
        &self,
        expression: HirExprId,
        callable: &CallableId,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<Option<etas_code::syntax::SyntaxBlockRef>, ProjectionError> {
        let access = self
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(ProjectionError::MissingReflection(expression))?
            .access_module;
        self.retained_body_at_access(callable, access, syntax)
    }

    pub(super) fn retained_body_at_access(
        &self,
        callable: &CallableId,
        access: Option<HirModuleId>,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<Option<etas_code::syntax::SyntaxBlockRef>, ProjectionError> {
        if let Some(external) = self.external_callables.get(callable) {
            let Some(body) = &external.body else {
                return Ok(None);
            };
            let caller = access.and_then(|module| self.identities.package_for_module(module));
            let owner = callable
                .as_declaration()
                .map(|key| key.package())
                .or_else(|| {
                    callable
                        .as_method()
                        .map(|method| method.implementation().package())
                })
                .ok_or_else(|| {
                    ProjectionError::Invalid("callable identity has no package owner".into())
                })?;
            if body.access == etas_code::descriptor::BodyAccess::Package && caller != Some(owner) {
                return Ok(None);
            }
            return syntax
                .import_retained_body(&external.canonical, &body.source)
                .map(Some)
                .map_err(ProjectionError::Syntax);
        }
        let item = self.item_owner(callable).ok_or_else(|| {
            ProjectionError::MissingIdentity("retained callable body owner".into())
        })?;
        let exposure = self.checked.definition_sources.exposure(item);
        if !exposure.retained {
            return Ok(None);
        }
        let caller = access.and_then(|module| self.identities.package_for_module(module));
        let owner = if let Some(method) = callable.as_method() {
            method.implementation().package()
        } else {
            callable
                .as_declaration()
                .ok_or_else(|| ProjectionError::Invalid("callable identity has no owner".into()))?
                .package()
        };
        if caller != Some(owner) && !exposure.exported {
            return Ok(None);
        }
        if let Some(revision) = self.checked.definition_sources.overlaid_body(item) {
            return Ok(Some(revision.syntax.clone()));
        }
        let Some(body) = self.checked.definition_sources.body(item) else {
            return Ok(None);
        };
        let source = self
            .checked
            .definition_sources
            .source(body.source)
            .ok_or_else(|| {
                ProjectionError::Invalid("retained callable has no native parsed source".into())
            })?;
        let owner = self
            .callables
            .get(callable)
            .ok_or_else(|| ProjectionError::MissingIdentity("retained callable slot".into()))?;
        syntax
            .retain_source_body(
                owner.native.canonical.fingerprint(),
                source.file.clone(),
                source.program.clone(),
                etas_syntax::ProgramBodySelection {
                    item: body.item.index,
                    impl_member: body.impl_member,
                },
            )
            .map(Some)
            .map_err(ProjectionError::Syntax)
    }

    pub fn build(
        checked: CheckedProgram,
        origins: &DefinitionOrigins,
        limits: DescriptorLimits,
    ) -> Result<Self, ProjectionError> {
        super::collect::build(checked, origins, limits)
    }

    pub fn descriptors(&self) -> &DescriptorGraph {
        &self.graph
    }

    pub fn native_type_origins(&self) -> &[crate::code::descriptor::NativeTypeOrigin] {
        &self.type_origins
    }

    pub fn native_type_declarations(
        &self,
    ) -> impl Iterator<Item = (etas_types::TypeId, &DefinitionKey)> {
        super::relocation::native_type_declarations(&self.checked.type_store, &self.identities)
    }

    pub fn native_effect_identity(
        &self,
        tag: etas_effects::EffectTagId,
    ) -> Result<&DefinitionKey, ProjectionError> {
        self.identities.effect_tag(tag)
    }

    pub fn agent_component(&self, identity: &DefinitionKey) -> Option<etas_types::TypeId> {
        self.agents
            .get(identity)
            .map(|agent| agent.component)
            .or_else(|| {
                self.external_agents
                    .get(identity)
                    .map(|agent| agent.component)
            })
    }

    pub fn native_callables(&self) -> impl Iterator<Item = (&CallableId, HirItemId)> {
        self.callables.iter().filter_map(|(id, callable)| {
            (id == &callable.native.canonical).then_some((id, callable.native.item))
        })
    }

    pub fn native_agent_default(&self, item: HirItemId) -> Option<&CallableId> {
        let etas_hir::HirItem::Agent(agent) = self.checked.hir.items.get(item)? else {
            return None;
        };
        let identity = self.agent_bindings.get(&agent.symbol)?;
        self.agents.get(identity)?.header.default_entry.as_ref()
    }

    /// Only this snapshot's checked expression can choose an access scope and
    /// selected implementation. Caller-constructed descriptor data is not evidence.
    pub fn static_reflection(
        &self,
        expression: HirExprId,
    ) -> Result<StaticReflectionView<'_>, ProjectionError> {
        let fact = self
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(ProjectionError::MissingReflection(expression))?;
        self.reflection_view(fact, &fact.target)
    }

    /// Resolves the checked access scope for a runtime agent binding. The item
    /// must come from a value pinned to this exact checked program.
    pub fn agent_value_reflection(
        &self,
        expression: HirExprId,
        item: HirItemId,
    ) -> Result<StaticReflectionView<'_>, ProjectionError> {
        let fact = self
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(ProjectionError::MissingReflection(expression))?;
        let CheckedReflectionTarget::AgentValue {
            component, method, ..
        } = fact.target
        else {
            return Err(ProjectionError::Invalid(
                "runtime reflection requires a checked agent binding".into(),
            ));
        };
        let Some(etas_types::ItemSignature::Agent(signature)) =
            self.checked.types.item_signatures.get(&item)
        else {
            return Err(ProjectionError::Invalid(
                "runtime reflection target is not a checked agent".into(),
            ));
        };
        let Some(etas_hir::HirItem::Agent(agent)) = self.checked.hir.items.get(item) else {
            return Err(ProjectionError::Invalid(
                "runtime reflection target has no agent declaration".into(),
            ));
        };
        if signature.component != component {
            return Err(ProjectionError::Invalid(
                "runtime agent reflection has a different nominal identity".into(),
            ));
        }
        let target = match method {
            Some(declaration) => CheckedReflectionTarget::Callable { declaration },
            None => CheckedReflectionTarget::Agent {
                declaration: agent.symbol,
                component,
            },
        };
        self.reflection_view(fact, &target)
    }

    pub fn callable_value_reflection(
        &self,
        expression: HirExprId,
        item: HirItemId,
    ) -> Result<&CallableHeaderInfo, ProjectionError> {
        self.compatible_callable_value_reflection(expression, item)?
            .ok_or_else(|| {
                ProjectionError::Invalid(
                    "reflected flow implementation differs from its checked binding signature"
                        .into(),
                )
            })
    }

    /// Finds an eligible checked value view without treating a different F as
    /// a corrupt fact. Missing facts and invalid source identities still fail.
    pub fn compatible_callable_value_reflection(
        &self,
        expression: HirExprId,
        item: HirItemId,
    ) -> Result<Option<&CallableHeaderInfo>, ProjectionError> {
        let fact = self
            .checked
            .checked_reflections()
            .get(&expression)
            .ok_or(ProjectionError::MissingReflection(expression))?;
        let CheckedReflectionTarget::CallableValue { signature, .. } = fact.target else {
            return Err(ProjectionError::Invalid(
                "runtime reflection requires a checked ordinary-flow binding".into(),
            ));
        };
        let Some(etas_hir::HirItem::Flow(flow)) = self.checked.hir.items.get(item) else {
            return Err(ProjectionError::Invalid(
                "reflected callable value has no ordinary-flow declaration".into(),
            ));
        };
        if self.checked.types.agent_methods.contains_key(&flow.symbol) {
            return Err(ProjectionError::Invalid(
                "an agent method is not an editable ordinary flow".into(),
            ));
        }
        let Some(etas_types::Type::Function(expected)) = self.checked.type_store.get(signature)
        else {
            return Err(ProjectionError::MissingType(signature));
        };
        let Some(etas_types::ItemSignature::Flow(actual)) =
            self.checked.types.item_signatures.get(&item)
        else {
            return Err(ProjectionError::MissingCallable(flow.symbol));
        };
        if expected.input != actual.params
            || expected.output != actual.output
            || expected.effects != actual.effects
        {
            return Ok(None);
        }
        let id = self
            .callable_bindings
            .get(&flow.symbol)
            .ok_or(ProjectionError::MissingCallable(flow.symbol))?;
        match self.callable_view(id, fact.access_module)? {
            StaticReflectionView::Callable(header) => Ok(Some(header)),
            StaticReflectionView::Agent(_) => Err(ProjectionError::Invalid(
                "ordinary-flow reflection resolved to an agent".into(),
            )),
        }
    }

    pub fn retained_agent_value_body(
        &self,
        expression: HirExprId,
        item: HirItemId,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<Option<etas_code::syntax::SyntaxBlockRef>, ProjectionError> {
        let callable = match self.agent_value_reflection(expression, item)? {
            StaticReflectionView::Agent(agent) => agent.default_entry,
            StaticReflectionView::Callable(callable) => Some(callable.id.clone()),
        };
        match callable {
            Some(callable) => self.retained_callable_body(expression, &callable, syntax),
            None => Ok(None),
        }
    }

    pub fn retained_callable_value_body(
        &self,
        expression: HirExprId,
        item: HirItemId,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<Option<etas_code::syntax::SyntaxBlockRef>, ProjectionError> {
        let header = self.callable_value_reflection(expression, item)?;
        self.retained_callable_body(expression, &header.id, syntax)
    }

    fn reflection_view(
        &self,
        fact: &etas_types::CheckedReflectionFact,
        target: &CheckedReflectionTarget,
    ) -> Result<StaticReflectionView<'_>, ProjectionError> {
        match *target {
            CheckedReflectionTarget::Agent {
                declaration,
                component,
            } => {
                let identity = self
                    .agent_bindings
                    .get(&declaration)
                    .ok_or(ProjectionError::MissingDeclaration(declaration))?;
                if let Some(agent) = self.external_agents.get(identity) {
                    if component != agent.component {
                        return Err(ProjectionError::Invalid(
                            "external agent has inconsistent nominal component facts".into(),
                        ));
                    }
                    return Ok(StaticReflectionView::Agent(agent.header.clone()));
                }
                let agent = self
                    .agents
                    .get(identity)
                    .ok_or(ProjectionError::MissingDeclaration(declaration))?;
                if component != agent.component {
                    return Err(ProjectionError::Invalid(
                        "reflected agent has inconsistent nominal component facts".into(),
                    ));
                }
                check_access(agent.private_to, fact.access_module)?;
                let mut view = agent.header.clone();
                let mut visible = Vec::with_capacity(view.methods.len());
                for method in &view.methods {
                    let callable = self
                        .callables
                        .get(method)
                        .ok_or(ProjectionError::MissingCallable(declaration))?;
                    if check_access(callable.native.private_to, fact.access_module).is_ok() {
                        visible.push(method.clone());
                    }
                }
                let visible = visible
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>();
                view.retain_additional_methods(|id| visible.contains(id));
                Ok(StaticReflectionView::Agent(view))
            }
            CheckedReflectionTarget::Callable { declaration } => {
                let selected = fact
                    .selected_spec
                    .as_ref()
                    .map(|bound| match &bound.spec {
                        CheckedSpecRef::Source(symbol) => self
                            .identities
                            .symbol(*symbol)
                            .ok_or(ProjectionError::MissingDeclaration(*symbol)),
                        CheckedSpecRef::Std(path)
                        | CheckedSpecRef::External(
                            etas_types::facts::specs::ExternalSpecIdentity { path, .. },
                        ) => self
                            .identities
                            .path(path)
                            .ok_or_else(|| ProjectionError::MissingIdentity(path.join("."))),
                    })
                    .transpose()?;
                let id = match selected {
                    Some(spec) => self.selected_bindings.get(&(declaration, spec.clone())),
                    None => self.callable_bindings.get(&declaration),
                }
                .ok_or(ProjectionError::MissingCallable(declaration))?;
                self.callable_view(id, fact.access_module)
            }
            CheckedReflectionTarget::ExternalCallable {
                ref callable,
                ref selected_spec,
            } => {
                let id = selected_spec
                    .as_ref()
                    .map(|spec| selected_method_id(callable, spec))
                    .transpose()?
                    .unwrap_or_else(|| callable.clone());
                if !self.external_callables.contains_key(&id) {
                    return Err(ProjectionError::Invalid(
                        "external reflected method has no published callable header".into(),
                    ));
                }
                self.callable_view(&id, fact.access_module)
            }
            CheckedReflectionTarget::AgentDefaultEntry {
                declaration,
                component,
            } => {
                let identity = self
                    .agent_bindings
                    .get(&declaration)
                    .ok_or(ProjectionError::MissingDeclaration(declaration))?;
                let agent = self
                    .agents
                    .get(identity)
                    .ok_or(ProjectionError::MissingDeclaration(declaration))?;
                if component != agent.component {
                    return Err(ProjectionError::Invalid(
                        "reflected default method has inconsistent nominal component facts".into(),
                    ));
                }
                let id = agent
                    .header
                    .default_entry
                    .as_ref()
                    .ok_or(ProjectionError::MissingCallable(declaration))?;
                self.callable_view(id, fact.access_module)
            }
            CheckedReflectionTarget::AgentValue { .. }
            | CheckedReflectionTarget::CallableValue { .. } => {
                Err(ProjectionError::RuntimeRevisionRequired)
            }
        }
    }

    pub(super) fn callable_view(
        &self,
        id: &CallableId,
        access: Option<HirModuleId>,
    ) -> Result<StaticReflectionView<'_>, ProjectionError> {
        if let Some(record) = self.external_callables.get(id) {
            return Ok(StaticReflectionView::Callable(&record.header));
        }
        let record = self
            .callables
            .get(id)
            .ok_or_else(|| ProjectionError::MissingIdentity("catalog callable".into()))?;
        check_access(record.native.private_to, access)?;
        Ok(StaticReflectionView::Callable(&record.header))
    }

    pub fn item_owner(&self, callable: &CallableId) -> Option<HirItemId> {
        self.callables
            .get(callable)
            .map(|record| record.native.item)
    }

    pub fn ordinary_flow_identity(&self, item: HirItemId) -> Option<&CallableId> {
        let etas_hir::HirItem::Flow(flow) = self.checked.hir.items.get(item)? else {
            return None;
        };
        if self.checked.types.agent_methods.contains_key(&flow.symbol) {
            return None;
        }
        self.callable_bindings.get(&flow.symbol)
    }

    pub fn callable(&self, callable: &CallableId) -> Option<&CallableHeaderInfo> {
        match self.callables.get(callable) {
            Some(record) => Some(&record.header),
            None => self
                .external_callables
                .get(callable)
                .map(|record| &record.header),
        }
    }

    pub fn external_callable(
        &self,
        callable: &CallableId,
    ) -> Option<(&CallableHeaderInfo, &DescriptorGraph)> {
        self.external_callables
            .get(callable)
            .map(|record| (&record.header, &record.graph))
    }

    pub fn is_dependency_source_callable(&self, callable: &CallableId) -> bool {
        self.item_owner(callable)
            .and_then(|item| self.checked.definition_sources.body(item))
            .and_then(|body| self.checked.definition_sources.source(body.source))
            .is_some_and(|source| {
                matches!(
                    source.kind,
                    crate::SourceKind::DependencySourceOverlay { .. }
                )
            })
    }

    pub fn callable_descriptors(&self, callable: &CallableId) -> Option<&DescriptorGraph> {
        if self.callables.contains_key(callable) {
            Some(&self.graph)
        } else {
            self.external_callables
                .get(callable)
                .map(|record| &record.graph)
        }
    }

    pub fn canonical_callable(&self, callable: &CallableId) -> Option<&CallableId> {
        self.callables
            .get(callable)
            .map(|record| &record.native.canonical)
            .or_else(|| {
                self.external_callables
                    .get(callable)
                    .map(|record| &record.canonical)
            })
    }

    pub(super) fn body_content(
        &self,
        callable: &CallableId,
        syntax: &crate::code::service::NativeCodeSyntaxOperations,
    ) -> Result<CodeFingerprint, ProjectionError> {
        let item = self
            .item_owner(callable)
            .ok_or_else(|| ProjectionError::MissingIdentity("definition body owner".into()))?;
        if let Some(revision) = self.checked.definition_sources.overlaid_body(item) {
            return Ok(revision.syntax.identity().content());
        }
        let body = self.checked.definition_sources.body(item).ok_or_else(|| {
            ProjectionError::MissingIdentity("definition body source slot".into())
        })?;
        let source = self
            .checked
            .definition_sources
            .source(body.source)
            .ok_or_else(|| ProjectionError::MissingIdentity("definition parsed source".into()))?;
        syntax
            .source_body_identity(
                callable.fingerprint(),
                source.file.clone(),
                source.program.clone(),
                etas_syntax::ProgramBodySelection {
                    item: body.item.index,
                    impl_member: body.impl_member,
                },
            )
            .map(|identity| identity.content())
            .map_err(ProjectionError::Syntax)
    }

    pub(super) fn add_selected_views(&mut self) -> Result<(), ProjectionError> {
        for fact in self.checked.checked_reflections().values() {
            if let CheckedReflectionTarget::ExternalCallable {
                callable,
                selected_spec: Some(spec),
            } = &fact.target
            {
                let mut record =
                    self.external_callables
                        .get(callable)
                        .cloned()
                        .ok_or_else(|| {
                            ProjectionError::MissingIdentity(
                                "selected external method has no canonical callable".into(),
                            )
                        })?;
                let subject = callable
                    .as_method()
                    .ok_or_else(|| {
                        ProjectionError::Invalid(
                            "selected external callable has no method owner".into(),
                        )
                    })?
                    .implementation()
                    .subject();
                let agent = self.external_agents.get(subject).ok_or_else(|| {
                    ProjectionError::MissingIdentity(
                        "selected external method has no agent owner".into(),
                    )
                })?;
                if !agent.header.method_info.as_ref().is_some_and(|methods| {
                    methods.iter().any(|method| {
                        method.callable == *callable
                            && method.is_public
                            && method.specs.iter().any(|bound| bound.definition == *spec)
                    })
                }) {
                    return Err(ProjectionError::Invalid(
                        "selected external method has no published spec binding".into(),
                    ));
                }
                record.header.id = selected_method_id(callable, spec)?;
                if let Some(previous) = self
                    .external_callables
                    .insert(record.header.id.clone(), record.clone())
                    && previous != record
                {
                    return Err(ProjectionError::Invalid(
                        "selected external method has conflicting canonical owners".into(),
                    ));
                }
                continue;
            }
            let Some(bound) = &fact.selected_spec else {
                continue;
            };
            let symbol = match fact.target {
                CheckedReflectionTarget::Callable { declaration } => declaration,
                CheckedReflectionTarget::AgentValue {
                    method: Some(method),
                    ..
                } => method,
                _ => {
                    return Err(ProjectionError::Invalid(
                        "selected spec reflection has no checked method target".into(),
                    ));
                }
            };
            let spec = match &bound.spec {
                CheckedSpecRef::Source(symbol) => self
                    .identities
                    .symbol(*symbol)
                    .ok_or(ProjectionError::MissingDeclaration(*symbol))?,
                CheckedSpecRef::Std(path)
                | CheckedSpecRef::External(etas_types::facts::specs::ExternalSpecIdentity {
                    path,
                    ..
                }) => self
                    .identities
                    .path(path)
                    .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))?,
            }
            .clone();
            let id = self
                .callable_bindings
                .get(&symbol)
                .ok_or(ProjectionError::MissingCallable(symbol))?;
            let method = id
                .as_method()
                .ok_or(ProjectionError::MissingCallable(symbol))?;
            let selected = method.select_spec(spec.clone())?;
            let selected = CallableId::method(selected);
            let record = self
                .callables
                .get(id)
                .ok_or(ProjectionError::MissingCallable(symbol))?;
            let record = CatalogCallable {
                header: CallableHeaderInfo {
                    id: selected.clone(),
                    signature: record.header.signature,
                    analysis: record.header.analysis.clone(),
                },
                native: record.native.clone(),
            };
            if let Some(previous) = self.callables.insert(selected.clone(), record)
                && previous.native.item
                    != self
                        .checked
                        .types
                        .agent_methods
                        .get(&symbol)
                        .ok_or(ProjectionError::MissingCallable(symbol))?
                        .body
            {
                return Err(ProjectionError::Invalid(
                    "selected callable identity has conflicting body owners".into(),
                ));
            }
            self.selected_bindings.insert((symbol, spec), selected);
        }
        Ok(())
    }
}

fn selected_method_id(
    callable: &CallableId,
    spec: &DefinitionKey,
) -> Result<CallableId, ProjectionError> {
    let method = callable.as_method().ok_or_else(|| {
        ProjectionError::Invalid("spec selection requires a method identity".into())
    })?;
    Ok(CallableId::method(method.select_spec(spec.clone())?))
}

pub(super) fn check_access(
    private_to: Option<HirModuleId>,
    access: Option<HirModuleId>,
) -> Result<(), ProjectionError> {
    if private_to.is_some() && private_to != access {
        Err(ProjectionError::InaccessibleDefinition)
    } else {
        Ok(())
    }
}
