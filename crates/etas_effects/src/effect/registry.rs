use std::collections::BTreeMap;

use super::{
    ActionRef, CoreEffect, EffectActionArgKind, EffectActionId, EffectActionSig, EffectTag,
    EffectTagId, ExtensionGraph,
};
use crate::{EffectPipelineError, RuntimeRequirementReason};
use etas_hir::{HirEffectDecl, HirItem, HirProgram, ResolveResult, SymbolDef, SymbolId};
use etas_std::{StdDecl, StdPrimitiveType, StdRegistry, StdSymbolKind, StdType, standard_registry};

pub const AGENTIC_TAG: EffectTagId = EffectTagId(0);
pub const NETWORK_TAG: EffectTagId = EffectTagId(1);
pub const FILE_IO_TAG: EffectTagId = EffectTagId(2);
pub const COMMAND_TAG: EffectTagId = EffectTagId(3);
pub const MEMORY_TAG: EffectTagId = EffectTagId(4);
pub const APPROVAL_TAG: EffectTagId = EffectTagId(5);
pub const SECRET_TAG: EffectTagId = EffectTagId(6);
pub const TIME_TAG: EffectTagId = EffectTagId(7);
pub const ERROR_TAG: EffectTagId = EffectTagId(8);
pub const CONSOLE_TAG: EffectTagId = EffectTagId(9);
pub const HUMAN_TAG: EffectTagId = EffectTagId(10);

pub const MEMORY_READ_ACTION: EffectActionId = EffectActionId(0);
pub const MEMORY_WRITE_ACTION: EffectActionId = EffectActionId(1);
pub const CONSOLE_STDIN_READ_LINE_ACTION: EffectActionId = EffectActionId(16);
pub const CONSOLE_STDIN_READ_ALL_ACTION: EffectActionId = EffectActionId(17);
pub const CONSOLE_STDOUT_WRITE_ACTION: EffectActionId = EffectActionId(18);
pub const CONSOLE_STDERR_WRITE_ACTION: EffectActionId = EffectActionId(19);
pub const APPROVAL_REQUEST_ACTION: EffectActionId = EffectActionId(32);
pub const ERROR_RAISE_ACTION: EffectActionId = EffectActionId(33);
pub const COMMAND_RUN_ACTION: EffectActionId = EffectActionId(83);
pub const AGENTIC_INFER_ACTION: EffectActionId = EffectActionId(88);

const FIRST_DYNAMIC_TAG: u32 = 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DependencyEffectMetadata {
    pub tags: Vec<DependencyEffectTag>,
    #[serde(default)]
    pub actions: Vec<DependencyEffectAction>,
    pub extensions: Vec<DependencyEffectExtension>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolProviderBindingMetadata {
    pub tool: Vec<String>,
    pub provider: String,
    pub effect_row: Vec<String>,
    #[serde(default)]
    pub action_row: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DependencyEffectTag {
    pub path: Vec<String>,
    pub runtime_requirement: Option<RuntimeRequirementReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DependencyEffectAction {
    pub path: Vec<String>,
    pub effect_args: Vec<EffectActionArgKind>,
    #[serde(default)]
    pub selector_param_names: Vec<String>,
    #[serde(default)]
    pub selector_defaults: Vec<Option<etas_types::EffectArgRef>>,
    pub returns_never: bool,
    pub runtime_requirement: Option<RuntimeRequirementReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DependencyEffectExtension {
    #[serde(default)]
    pub package: Option<u32>,
    pub child: Vec<String>,
    pub parent: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnresolvedDependencyEffectExtension {
    pub package: Option<u32>,
    pub child: Vec<String>,
    pub parent: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectRegistry {
    tags: BTreeMap<EffectTagId, EffectTag>,
    names: BTreeMap<String, EffectTagId>,
    symbol_tags: BTreeMap<SymbolId, EffectTagId>,
    actions: BTreeMap<SymbolId, EffectActionSig>,
    dependency_actions: BTreeMap<EffectActionId, EffectActionSig>,
    #[serde(default)]
    dependency_tag_paths: BTreeMap<EffectTagId, Vec<String>>,
    #[serde(default)]
    dependency_action_paths: BTreeMap<ActionRef, Vec<String>>,
    standard_actions: BTreeMap<EffectActionId, EffectActionSig>,
    action_names: BTreeMap<String, ActionRef>,
    actions_by_owner: BTreeMap<EffectTagId, Vec<SymbolId>>,
    extensions: ExtensionGraph,
    memory_places: BTreeMap<etas_types::TypeId, MemoryPlaceDecl>,
    unresolved_dependency_extensions: Vec<UnresolvedDependencyEffectExtension>,
}

fn invalid_dependency_metadata(reason: impl Into<String>) -> EffectPipelineError {
    EffectPipelineError::InvalidExternalMetadata {
        package: "dependency effect metadata".to_owned(),
        reason: reason.into(),
    }
}

impl EffectRegistry {
    pub fn from_hir_and_types(
        hir: &HirProgram,
        types: &etas_types::TypeOutput,
    ) -> Result<Self, EffectPipelineError> {
        Self::from_hir_types_and_dependencies(hir, types, &DependencyEffectMetadata::default())
    }

    pub fn from_hir_types_and_dependencies(
        hir: &HirProgram,
        types: &etas_types::TypeOutput,
        dependency_metadata: &DependencyEffectMetadata,
    ) -> Result<Self, EffectPipelineError> {
        let std_registry = standard_registry();
        Self::from_hir_types_dependencies_and_std(hir, types, dependency_metadata, &std_registry)
    }

    pub fn from_hir_types_dependencies_and_std(
        hir: &HirProgram,
        types: &etas_types::TypeOutput,
        dependency_metadata: &DependencyEffectMetadata,
        std_registry: &StdRegistry,
    ) -> Result<Self, EffectPipelineError> {
        let mut registry = Self::with_standard_effects_from(std_registry);
        registry.register_dependency_metadata(dependency_metadata)?;
        registry.register_hir_effects(hir);
        registry.register_hir_effect_extensions(hir);
        registry.register_hir_actions(hir, &types.facts);
        registry.register_memory_places(types.store.iter());
        Ok(registry)
    }

    pub fn with_standard_effects() -> Self {
        let std_registry = standard_registry();
        Self::with_standard_effects_from(&std_registry)
    }

    pub fn with_standard_effects_from(std_registry: &StdRegistry) -> Self {
        let mut registry = Self::default();
        registry.register_core("Agentic", AGENTIC_TAG, CoreEffect::Agentic);
        registry.register_core("Network", NETWORK_TAG, CoreEffect::Network);
        registry.register_core("FileIO", FILE_IO_TAG, CoreEffect::FileIO);
        registry.register_core("Command", COMMAND_TAG, CoreEffect::Command);
        registry.register_core("Memory", MEMORY_TAG, CoreEffect::Memory);
        registry.register_core("Secret", SECRET_TAG, CoreEffect::Secret);
        registry.register_core("Time", TIME_TAG, CoreEffect::Time);
        registry.register_core("Human", HUMAN_TAG, CoreEffect::Human);
        registry.register_core("Error", ERROR_TAG, CoreEffect::Error);
        registry.register_standard_library_descriptors(std_registry);
        registry
    }

    pub fn with_standard_effects_and_memory_places(types: &etas_types::TypeStore) -> Self {
        let std_registry = standard_registry();
        let mut registry = Self::with_standard_effects_from(&std_registry);
        registry.register_memory_places(types.iter());
        registry
    }

    pub fn tag_by_name(&self, name: &str) -> Option<EffectTagId> {
        self.names.get(name).copied()
    }

    pub fn tags(&self) -> impl Iterator<Item = &EffectTag> {
        self.tags.values()
    }

    pub fn tag_by_symbol(&self, symbol: SymbolId) -> Option<EffectTagId> {
        self.symbol_tags.get(&symbol).copied()
    }

    pub fn tag_by_core(&self, core: CoreEffect) -> Option<EffectTagId> {
        self.tags
            .values()
            .find_map(|tag| (tag.core == Some(core)).then_some(tag.id))
    }

    pub fn tag_name(&self, tag: EffectTagId) -> Option<&str> {
        self.tags.get(&tag).map(|tag| tag.name.as_str())
    }

    pub fn dependency_tag_path(&self, tag: EffectTagId) -> Option<&[String]> {
        self.dependency_tag_paths.get(&tag).map(Vec::as_slice)
    }

    pub fn dependency_action_path(&self, action: &ActionRef) -> Option<&[String]> {
        self.dependency_action_paths.get(action).map(Vec::as_slice)
    }

    pub fn tag_extends(&self, child: EffectTagId, ancestor: EffectTagId) -> bool {
        self.extensions.extends(child, ancestor)
    }

    pub fn unresolved_dependency_extensions(&self) -> &[UnresolvedDependencyEffectExtension] {
        &self.unresolved_dependency_extensions
    }

    pub fn action(&self, symbol: SymbolId) -> Option<&EffectActionSig> {
        self.actions.get(&symbol)
    }

    pub fn standard_action(&self, action: EffectActionId) -> Option<&EffectActionSig> {
        self.standard_actions.get(&action)
    }

    pub fn action_signature(&self, action: &ActionRef) -> Option<&EffectActionSig> {
        self.standard_actions
            .get(&action.action)
            .filter(|signature| signature.owner == action.tag)
            .or_else(|| self.local_action_signature(action))
            .or_else(|| self.dependency_action_signature(action))
    }

    pub fn action_name(&self, owner: EffectTagId, action: EffectActionId) -> Option<&str> {
        let action_ref = ActionRef { tag: owner, action };
        self.standard_actions
            .get(&action_ref.action)
            .filter(|signature| signature.owner == action_ref.tag)
            .or_else(|| self.local_action_signature(&action_ref))
            .or_else(|| self.dependency_action_signature(&action_ref))
            .map(|signature| signature.name.as_str())
    }

    pub fn action_by_name(&self, name: &str) -> Option<ActionRef> {
        self.action_names.get(name).cloned()
    }

    pub fn standard_action_by_name(&self, name: &str) -> Option<ActionRef> {
        let action = self.action_by_name(name)?;
        self.standard_actions
            .get(&action.action)
            .filter(|signature| signature.owner == action.tag)
            .map(|_| action)
    }

    pub fn standard_tag_by_name(&self, name: &str) -> Option<EffectTagId> {
        let tag = self.tag_by_name(name)?;
        (tag.0 < FIRST_DYNAMIC_TAG && !self.dependency_tag_paths.contains_key(&tag)).then_some(tag)
    }

    pub fn core_action(&self, core: CoreEffect, action: EffectActionId) -> Option<ActionRef> {
        let tag = self.tag_by_core(core)?;
        self.standard_actions
            .get(&action)
            .filter(|signature| signature.owner == tag)
            .map(|_| ActionRef { tag, action })
    }

    pub fn action_names_for_owner(&self, owner: &str) -> Vec<String> {
        let prefix = format!("{owner}.");
        self.action_names
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect()
    }

    pub fn runtime_requirement_reason_for_action(
        &self,
        action: EffectActionId,
    ) -> Option<RuntimeRequirementReason> {
        self.standard_actions
            .get(&action)
            .or_else(|| self.dependency_actions.get(&action))
            .and_then(|action| action.runtime_requirement.clone())
    }

    pub fn runtime_requirement_reason_for_action_ref(
        &self,
        action: &ActionRef,
    ) -> Option<RuntimeRequirementReason> {
        self.standard_actions
            .get(&action.action)
            .filter(|signature| signature.owner == action.tag)
            .or_else(|| self.local_action_signature(action))
            .or_else(|| self.dependency_action_signature(action))
            .and_then(|signature| signature.runtime_requirement.clone())
    }

    pub fn tag_requires_high_impact_ack(&self, tag: EffectTagId) -> bool {
        self.tags.get(&tag).is_some_and(|tag| tag.high_impact_ack)
    }

    pub fn action_requires_high_impact_ack(&self, action: &ActionRef) -> bool {
        self.standard_actions
            .get(&action.action)
            .filter(|signature| signature.owner == action.tag)
            .or_else(|| self.local_action_signature(action))
            .or_else(|| self.dependency_action_signature(action))
            .is_some_and(|signature| signature.high_impact_ack)
    }

    pub fn actions_by_owner(&self, owner: EffectTagId) -> Option<&[SymbolId]> {
        self.actions_by_owner.get(&owner).map(Vec::as_slice)
    }

    fn local_action_signature(&self, action: &ActionRef) -> Option<&EffectActionSig> {
        self.actions
            .values()
            .find(|signature| signature.owner == action.tag && signature.id == action.action)
    }

    fn dependency_action_signature(&self, action: &ActionRef) -> Option<&EffectActionSig> {
        self.dependency_actions
            .get(&action.action)
            .filter(|signature| signature.owner == action.tag)
    }

    pub fn memory_place(&self, place: etas_types::TypeId) -> Option<&MemoryPlaceDecl> {
        self.memory_places.get(&place)
    }

    pub fn memory_place_contains(
        &self,
        parent: etas_types::TypeId,
        child: etas_types::TypeId,
    ) -> bool {
        let Some(parent) = self.memory_place(parent) else {
            return false;
        };
        let Some(child) = self.memory_place(child) else {
            return false;
        };
        child.segments.starts_with(&parent.segments)
    }

    pub fn is_memory_tag(&self, tag: EffectTagId) -> bool {
        matches!(
            self.tags.get(&tag).and_then(|tag| tag.core),
            Some(CoreEffect::Memory)
        )
    }

    pub fn runtime_requirement_reason(&self, tag: EffectTagId) -> Option<RuntimeRequirementReason> {
        self.tags
            .get(&tag)
            .and_then(|tag| tag.runtime_requirement.clone())
    }

    fn register_core(&mut self, name: &str, id: EffectTagId, core: CoreEffect) {
        self.register_tag(id, name, Some(core), runtime_requirement_for_core(core));
        self.names.insert(format!("std.runtime.effects.{name}"), id);
    }

    fn register_standard_runtime(
        &mut self,
        name: &str,
        id: EffectTagId,
        runtime_requirement: RuntimeRequirementReason,
        high_impact_ack: bool,
    ) {
        self.register_tag(id, name, None, Some(runtime_requirement));
        if high_impact_ack {
            self.tags
                .entry(id)
                .and_modify(|tag| tag.high_impact_ack = true);
        }
    }

    fn register_standard_library_descriptors(&mut self, std_registry: &StdRegistry) {
        for symbol in std_registry.symbols() {
            if symbol.kind != StdSymbolKind::Effect || !is_std_effect_symbol(&symbol.qualified_path)
            {
                continue;
            }
            let StdDecl::Effect(effect) = &symbol.decl else {
                continue;
            };
            if effect.core {
                continue;
            }
            let Some(stable_id) = effect.stable_id else {
                continue;
            };
            let Some(runtime_requirement) = effect
                .runtime_requirement
                .as_ref()
                .map(runtime_requirement_from_std)
            else {
                continue;
            };
            let id = EffectTagId(stable_id);
            self.register_standard_runtime(
                &effect.name,
                id,
                runtime_requirement,
                effect.high_impact_ack,
            );
            for parent in &effect.extends {
                let Some(parent) = self.tag_by_name(parent) else {
                    continue;
                };
                self.add_extension(id, parent);
            }
        }

        for symbol in std_registry.symbols() {
            if symbol.kind != StdSymbolKind::EffectAction
                || !is_std_effect_action_symbol(&symbol.qualified_path)
            {
                continue;
            }
            let StdDecl::EffectAction(action) = &symbol.decl else {
                continue;
            };
            let Some(stable_id) = action.stable_id else {
                continue;
            };
            let Some(owner) = self.tag_by_name(&action.owner) else {
                continue;
            };
            let runtime_requirement = action
                .runtime_requirement
                .as_ref()
                .map(runtime_requirement_from_std)
                .or_else(|| self.runtime_requirement_reason(owner));
            self.register_standard_action(
                StandardActionRegistration {
                    id: EffectActionId(stable_id),
                    owner,
                    name: &action.name,
                    runtime_requirement,
                    effect_args: effect_action_args_from_std_decl(action),
                    returns_never: action_returns_never(&action.output),
                    high_impact_ack: action.high_impact_ack,
                },
                std_registry,
            );
        }

        if let Some(symbol) = std_registry.lookup_qualified(&["std", "runtime", "error", "raise"]) {
            if let StdDecl::EffectAction(action) = &symbol.decl {
                self.register_standard_local_action(
                    StandardActionRegistration {
                        id: action
                            .stable_id
                            .map(EffectActionId)
                            .unwrap_or(ERROR_RAISE_ACTION),
                        owner: ERROR_TAG,
                        name: &action.name,
                        runtime_requirement: action
                            .runtime_requirement
                            .as_ref()
                            .map(runtime_requirement_from_std),
                        effect_args: effect_action_args_from_std_decl(action),
                        returns_never: action_returns_never(&action.output),
                        high_impact_ack: action.high_impact_ack,
                    },
                    std_registry,
                );
            }
        }
    }

    fn register_standard_action(
        &mut self,
        registration: StandardActionRegistration<'_>,
        std_registry: &StdRegistry,
    ) {
        let runtime_requirement = registration.runtime_requirement.clone();
        let owner = registration.owner;
        self.register_standard_action_with_runtime(registration, std_registry);
        if let Some(runtime_requirement) = runtime_requirement {
            self.tags.entry(owner).and_modify(|tag| {
                tag.runtime_requirement = tag
                    .runtime_requirement
                    .clone()
                    .or(Some(runtime_requirement))
            });
        }
    }

    fn register_standard_local_action(
        &mut self,
        registration: StandardActionRegistration<'_>,
        std_registry: &StdRegistry,
    ) {
        self.register_standard_action_with_runtime(registration, std_registry);
    }

    fn register_standard_action_with_runtime(
        &mut self,
        registration: StandardActionRegistration<'_>,
        std_registry: &StdRegistry,
    ) {
        let StandardActionRegistration {
            id,
            owner,
            name,
            runtime_requirement,
            effect_args,
            returns_never,
            high_impact_ack,
        } = registration;
        let Some(owner_tag) = self.tags.get(&owner) else {
            return;
        };
        let owner_name = owner_tag.name.clone();
        let sig = EffectActionSig {
            id,
            owner,
            name: name.to_owned(),
            effect_args,
            selector_param_names: Vec::new(),
            selector_defaults: Vec::new(),
            params: Vec::new(),
            output: etas_types::TypeId(0),
            returns_never,
            runtime_requirement,
            high_impact_ack,
        };
        self.standard_actions.insert(id, sig);
        let action_ref = ActionRef {
            tag: owner,
            action: id,
        };
        self.action_names
            .insert(format!("{}.{}", owner_name, name), action_ref.clone());
        self.action_names.insert(
            format!("std.effects.{}.{}", owner_name, name),
            action_ref.clone(),
        );
        self.action_names.insert(
            format!("std.effects.actions.{}.{}", owner_name, name),
            action_ref.clone(),
        );
        self.action_names.insert(
            format!("std.runtime.effects.{}.{}", owner_name, name),
            action_ref.clone(),
        );
        self.register_standard_action_owner_symbol_aliases(
            owner_name.as_str(),
            name,
            action_ref,
            std_registry,
        );
    }

    fn register_standard_action_owner_symbol_aliases(
        &mut self,
        owner: &str,
        name: &str,
        action_ref: ActionRef,
        std_registry: &StdRegistry,
    ) {
        for symbol in std_registry.symbols() {
            if symbol
                .qualified_path
                .last()
                .is_some_and(|local| local == owner)
            {
                let mut path = symbol.qualified_path.clone();
                path.push(name.to_owned());
                self.action_names.insert(path.join("."), action_ref.clone());
            }
        }
    }

    fn register_tag(
        &mut self,
        id: EffectTagId,
        name: &str,
        core: Option<CoreEffect>,
        runtime_requirement: Option<RuntimeRequirementReason>,
    ) {
        self.tags.insert(
            id,
            EffectTag {
                id,
                name: name.to_owned(),
                core,
                runtime_requirement,
                high_impact_ack: core.is_some_and(core_effect_requires_high_impact_ack),
            },
        );
        self.names.insert(name.to_owned(), id);
        self.names.insert(format!("std.effects.{name}"), id);
    }

    fn register_hir_effects(&mut self, hir: &HirProgram) {
        let mut next_user_tag = self.next_dynamic_tag();
        for (_, item) in hir.items.iter() {
            let HirItem::Effect(effect) = item else {
                continue;
            };
            if let Some(existing) = self.tag_by_symbol(effect.symbol) {
                self.register_effect_names(hir, effect, existing);
                continue;
            }
            let Some(name) = symbol_name(hir, effect.symbol) else {
                continue;
            };
            if let Some(existing) = self
                .tag_by_name(name)
                .filter(|existing| self.can_source_effect_bind_standard_tag(name, *existing))
            {
                self.symbol_tags.insert(effect.symbol, existing);
                self.register_effect_names(hir, effect, existing);
                continue;
            }
            let id = EffectTagId(next_user_tag);
            next_user_tag += 1;
            self.symbol_tags.insert(effect.symbol, id);
            self.register_effect_names(hir, effect, id);
        }
    }

    fn register_dependency_metadata(
        &mut self,
        metadata: &DependencyEffectMetadata,
    ) -> Result<(), EffectPipelineError> {
        for tag in &metadata.tags {
            let Some(name) = tag.path.last().cloned() else {
                return Err(invalid_dependency_metadata(
                    "dependency effect tag path must not be empty",
                ));
            };
            let qualified = tag.path.join(".");
            if self.tag_by_name(&qualified).is_some() {
                continue;
            }
            let id = EffectTagId(self.next_dynamic_tag());
            self.tags.insert(
                id,
                EffectTag {
                    id,
                    name,
                    core: None,
                    runtime_requirement: tag.runtime_requirement.clone(),
                    high_impact_ack: false,
                },
            );
            self.names.insert(qualified, id);
            self.dependency_tag_paths.insert(id, tag.path.clone());
        }

        for action in &metadata.actions {
            if action.path.len() < 2 {
                return Err(invalid_dependency_metadata(format!(
                    "dependency action path `{}` must contain an effect and action name",
                    action.path.join(".")
                )));
            }
            if action.effect_args.len() != action.selector_param_names.len()
                || action.effect_args.len() != action.selector_defaults.len()
            {
                return Err(invalid_dependency_metadata(format!(
                    "dependency action `{}` selector metadata lengths do not match",
                    action.path.join(".")
                )));
            }
            let qualified = action.path.join(".");
            let owner_path = &action.path[..action.path.len() - 1];
            let Some(owner) = self.tag_by_path(owner_path) else {
                return Err(invalid_dependency_metadata(format!(
                    "dependency action `{}` references unknown effect owner `{}`",
                    action.path.join("."),
                    owner_path.join(".")
                )));
            };
            if self
                .action_by_name(&qualified)
                .is_some_and(|existing| existing.tag == owner)
            {
                continue;
            }
            let id = EffectActionId(self.next_dynamic_action());
            let Some(name) = action.path.last().cloned() else {
                return Err(invalid_dependency_metadata(
                    "dependency action path must not be empty",
                ));
            };
            let action_ref = ActionRef {
                tag: owner,
                action: id,
            };
            self.dependency_actions.insert(
                id,
                EffectActionSig {
                    id,
                    owner,
                    name,
                    effect_args: action.effect_args.clone(),
                    selector_param_names: action.selector_param_names.clone(),
                    selector_defaults: action.selector_defaults.clone(),
                    params: Vec::new(),
                    output: etas_types::TypeId(0),
                    returns_never: action.returns_never,
                    runtime_requirement: action
                        .runtime_requirement
                        .clone()
                        .or_else(|| self.runtime_requirement_reason(owner)),
                    high_impact_ack: false,
                },
            );
            self.action_names.insert(qualified, action_ref.clone());
            self.dependency_action_paths
                .insert(action_ref.clone(), action.path.clone());
            if action.path.len() > 2
                && let (Some(owner_name), Some(action_name)) =
                    (action.path.get(action.path.len() - 2), action.path.last())
            {
                self.action_names
                    .entry(format!("{owner_name}.{action_name}"))
                    .or_insert(ActionRef {
                        tag: owner,
                        action: id,
                    });
            }
        }

        for extension in &metadata.extensions {
            let Some(child) = self.tag_by_path(&extension.child) else {
                self.unresolved_dependency_extensions
                    .push(UnresolvedDependencyEffectExtension {
                        package: extension.package,
                        child: extension.child.clone(),
                        parent: extension.parent.clone(),
                    });
                continue;
            };
            let Some(parent) = self.tag_by_path(&extension.parent) else {
                self.unresolved_dependency_extensions
                    .push(UnresolvedDependencyEffectExtension {
                        package: extension.package,
                        child: extension.child.clone(),
                        parent: extension.parent.clone(),
                    });
                continue;
            };
            self.add_extension(child, parent);
        }
        Ok(())
    }

    fn register_effect_names(&mut self, hir: &HirProgram, effect: &HirEffectDecl, id: EffectTagId) {
        let Some(name) = symbol_name(hir, effect.symbol) else {
            return;
        };
        self.tags.entry(id).or_insert_with(|| EffectTag {
            id,
            name: name.to_owned(),
            core: None,
            runtime_requirement: None,
            high_impact_ack: false,
        });
        self.names.insert(name.to_owned(), id);
        if let Some(module) = module_name_for_symbol(hir, effect.symbol) {
            self.names.insert(format!("{module}.{name}"), id);
        }
    }

    fn can_source_effect_bind_standard_tag(&self, name: &str, id: EffectTagId) -> bool {
        let Some(tag) = self.tags.get(&id) else {
            return false;
        };
        tag.core.is_some() || matches!(name, "Approval" | "Console")
    }

    fn register_hir_effect_extensions(&mut self, hir: &HirProgram) {
        for (_, item) in hir.items.iter() {
            let HirItem::Effect(effect) = item else {
                continue;
            };
            let Some(child) = self.tag_by_symbol(effect.symbol) else {
                continue;
            };
            let Some(extends) = &effect.extends else {
                continue;
            };
            let Some(parent) = self.resolve_effect_ref(hir, &extends.path.resolution) else {
                continue;
            };
            self.add_extension(child, parent);
        }
    }

    fn register_hir_actions(&mut self, hir: &HirProgram, types: &etas_types::TypeFacts) {
        let mut next_user_action = self.next_dynamic_action();
        for (symbol, signature) in &types.action_signatures {
            let Some(owner) =
                action_owner_symbol(hir, *symbol).and_then(|owner| self.tag_by_symbol(owner))
            else {
                continue;
            };
            let Some(owner_name) = self.tags.get(&owner).map(|tag| tag.name.clone()) else {
                continue;
            };
            let Some(action_name) = symbol_name(hir, *symbol) else {
                continue;
            };
            let local_action_name = action_name
                .strip_prefix(owner_name.as_str())
                .and_then(|rest| rest.strip_prefix('.'))
                .unwrap_or(action_name);
            let action_id = self
                .action_by_name(&format!("{owner_name}.{local_action_name}"))
                .filter(|action| action.tag == owner)
                .map(|action| action.action)
                .unwrap_or_else(|| {
                    let id = EffectActionId(next_user_action);
                    next_user_action = next_user_action.saturating_add(1);
                    id
                });
            let sig = EffectActionSig {
                id: action_id,
                owner,
                name: local_action_name.to_owned(),
                effect_args: signature
                    .effect_args
                    .iter()
                    .map(effect_action_arg_kind_from_types)
                    .collect(),
                selector_param_names: signature.selector_param_names.clone(),
                selector_defaults: signature.selector_defaults.clone(),
                params: signature.params.clone(),
                output: signature.output,
                returns_never: signature.returns_never,
                runtime_requirement: self.runtime_requirement_reason(owner),
                high_impact_ack: false,
            };
            self.actions.insert(*symbol, sig);
            if let Some(owner_tag) = self.tags.get(&owner) {
                let action_ref = ActionRef {
                    tag: owner,
                    action: action_id,
                };
                self.action_names.insert(
                    format!("{}.{}", owner_tag.name, local_action_name),
                    action_ref.clone(),
                );
                if let Some(module) = module_name_for_symbol(hir, *symbol) {
                    self.action_names.insert(
                        format!("{module}.{}.{}", owner_tag.name, local_action_name),
                        action_ref,
                    );
                }
            }
            self.actions_by_owner
                .entry(owner)
                .or_default()
                .push(*symbol);
        }
    }

    fn register_memory_places<'a>(
        &mut self,
        types: impl Iterator<Item = (etas_types::TypeId, &'a etas_types::Type)>,
    ) {
        for (id, ty) in types {
            let etas_types::Type::MemoryPlace(place) = ty else {
                continue;
            };
            self.memory_places.insert(
                id,
                MemoryPlaceDecl {
                    id,
                    segments: place.segments.clone(),
                },
            );
        }
    }

    fn add_extension(&mut self, child: EffectTagId, parent: EffectTagId) {
        self.extensions.add_extension(child, parent);
        let parent_requirement = self.runtime_requirement_reason(parent);
        if let Some(parent_requirement) = parent_requirement {
            self.tags.entry(child).and_modify(|tag| {
                tag.runtime_requirement
                    .get_or_insert_with(|| parent_requirement.clone());
            });
        }
    }

    fn next_dynamic_tag(&self) -> u32 {
        self.tags
            .keys()
            .map(|id| id.0)
            .max()
            .unwrap_or(FIRST_DYNAMIC_TAG - 1)
            .saturating_add(1)
            .max(FIRST_DYNAMIC_TAG)
    }

    fn next_dynamic_action(&self) -> u32 {
        self.standard_actions
            .keys()
            .map(|id| id.0)
            .chain(self.dependency_actions.keys().map(|id| id.0))
            .chain(self.actions.values().map(|signature| signature.id.0))
            .max()
            .unwrap_or(FIRST_DYNAMIC_TAG - 1)
            .saturating_add(1)
            .max(FIRST_DYNAMIC_TAG)
    }

    fn tag_by_path(&self, path: &[String]) -> Option<EffectTagId> {
        let qualified = path.join(".");
        self.tag_by_name(&qualified).or_else(|| {
            (path.len() == 1)
                .then(|| self.tag_by_name(&path[0]))
                .flatten()
        })
    }

    fn resolve_effect_ref(
        &self,
        hir: &HirProgram,
        resolution: &ResolveResult,
    ) -> Option<EffectTagId> {
        match resolution {
            ResolveResult::Resolved(symbol) => self.tag_by_resolved_symbol(hir, *symbol),
            ResolveResult::PartiallyResolved(_)
            | ResolveResult::Unresolved
            | ResolveResult::Ambiguous(_) => None,
        }
    }

    fn tag_by_resolved_symbol(&self, hir: &HirProgram, symbol: SymbolId) -> Option<EffectTagId> {
        self.tag_by_symbol(symbol).or_else(|| {
            let SymbolDef::ImportAlias { path, .. } = &hir.symbols.get(symbol)?.def else {
                return None;
            };
            self.tag_by_name(&path.join("."))
        })
    }
}

struct StandardActionRegistration<'a> {
    id: EffectActionId,
    owner: EffectTagId,
    name: &'a str,
    runtime_requirement: Option<RuntimeRequirementReason>,
    effect_args: Vec<EffectActionArgKind>,
    returns_never: bool,
    high_impact_ack: bool,
}

fn effect_action_arg_kind_from_types(
    kind: &etas_types::EffectActionArgKind,
) -> EffectActionArgKind {
    match kind {
        etas_types::EffectActionArgKind::Type => EffectActionArgKind::Type,
        etas_types::EffectActionArgKind::MemoryPlace => EffectActionArgKind::MemoryPlace,
        etas_types::EffectActionArgKind::StaticResourcePath { ty } => {
            EffectActionArgKind::StaticResourcePath { ty: ty.clone() }
        }
        etas_types::EffectActionArgKind::StringPattern => EffectActionArgKind::StringPattern,
    }
}

fn effect_action_args_from_std_decl(
    action: &etas_std::EffectActionDecl,
) -> Vec<EffectActionArgKind> {
    action
        .effect_args
        .iter()
        .map(|kind| match kind {
            etas_std::EffectActionArgKind::Type => EffectActionArgKind::Type,
            etas_std::EffectActionArgKind::MemoryPlace => EffectActionArgKind::MemoryPlace,
            etas_std::EffectActionArgKind::StaticResourcePath { ty } => {
                EffectActionArgKind::StaticResourcePath {
                    ty: (*ty).to_owned(),
                }
            }
            etas_std::EffectActionArgKind::StringPattern => EffectActionArgKind::StringPattern,
        })
        .collect()
}

fn runtime_requirement_from_std(
    requirement: &etas_std::StdRuntimeRequirement,
) -> RuntimeRequirementReason {
    match requirement {
        etas_std::StdRuntimeRequirement::Agentic => RuntimeRequirementReason::Agentic,
        etas_std::StdRuntimeRequirement::ToolCall => RuntimeRequirementReason::ToolCall,
        etas_std::StdRuntimeRequirement::HostAuthority => RuntimeRequirementReason::HostAuthority,
        etas_std::StdRuntimeRequirement::DurableMemory => RuntimeRequirementReason::DurableMemory,
        etas_std::StdRuntimeRequirement::Approval => RuntimeRequirementReason::Approval,
        etas_std::StdRuntimeRequirement::Checkpoint => RuntimeRequirementReason::Checkpoint,
        etas_std::StdRuntimeRequirement::Time => RuntimeRequirementReason::Time,
        etas_std::StdRuntimeRequirement::Network => RuntimeRequirementReason::Network,
        etas_std::StdRuntimeRequirement::Tcp => RuntimeRequirementReason::Tcp,
        etas_std::StdRuntimeRequirement::Stream => RuntimeRequirementReason::Stream,
        etas_std::StdRuntimeRequirement::Tls => RuntimeRequirementReason::Tls,
        etas_std::StdRuntimeRequirement::Browser => RuntimeRequirementReason::Browser,
        etas_std::StdRuntimeRequirement::Console => RuntimeRequirementReason::Console,
        etas_std::StdRuntimeRequirement::FileIO => RuntimeRequirementReason::FileIO,
        etas_std::StdRuntimeRequirement::Command => RuntimeRequirementReason::Command,
        etas_std::StdRuntimeRequirement::SecretAccess => RuntimeRequirementReason::SecretAccess,
        etas_std::StdRuntimeRequirement::RuntimeHandler => RuntimeRequirementReason::RuntimeHandler,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryPlaceDecl {
    pub id: etas_types::TypeId,
    pub segments: Vec<String>,
}

fn runtime_requirement_for_core(core: CoreEffect) -> Option<RuntimeRequirementReason> {
    match core {
        CoreEffect::Agentic => Some(RuntimeRequirementReason::Agentic),
        CoreEffect::Network => Some(RuntimeRequirementReason::Network),
        CoreEffect::FileIO => Some(RuntimeRequirementReason::FileIO),
        CoreEffect::Command => Some(RuntimeRequirementReason::Command),
        CoreEffect::Memory => Some(RuntimeRequirementReason::DurableMemory),
        CoreEffect::Secret => Some(RuntimeRequirementReason::SecretAccess),
        CoreEffect::Time => Some(RuntimeRequirementReason::Time),
        CoreEffect::Human => None,
        CoreEffect::Error => None,
    }
}

fn core_effect_requires_high_impact_ack(core: CoreEffect) -> bool {
    matches!(core, CoreEffect::Command | CoreEffect::Secret)
}

fn is_std_effect_symbol(path: &[String]) -> bool {
    path.len() == 3 && path[0] == "std" && path[1] == "effects"
}

fn is_std_effect_action_symbol(path: &[String]) -> bool {
    path.len() == 5 && path[0] == "std" && path[1] == "effects" && path[2] == "actions"
}

fn action_returns_never(output: &StdType) -> bool {
    matches!(output, StdType::Primitive(StdPrimitiveType::Never))
}

fn action_owner_symbol(hir: &HirProgram, symbol: SymbolId) -> Option<SymbolId> {
    match &hir.symbols.get(symbol)?.def {
        SymbolDef::EffectAction { owner_effect, .. } => *owner_effect,
        _ => None,
    }
}

fn module_name_for_symbol(hir: &HirProgram, symbol: SymbolId) -> Option<String> {
    let item_id = hir.symbols.get(symbol)?.defining_item?;
    hir.modules
        .iter()
        .filter_map(|module_id| hir.modules_arena.get(*module_id))
        .find(|module| module.items.contains(&item_id))
        .and_then(|module| module.name.as_ref())
        .map(|name| {
            name.segments
                .iter()
                .map(|segment| segment.name.as_str())
                .collect::<Vec<_>>()
                .join(".")
        })
}

fn symbol_name(hir: &HirProgram, symbol: SymbolId) -> Option<&str> {
    hir.symbols.get(symbol).map(|symbol| symbol.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_action_metadata_registers_canonical_and_import_visible_names() {
        let metadata = DependencyEffectMetadata {
            tags: vec![DependencyEffectTag {
                path: vec!["errors".to_owned(), "Net".to_owned()],
                runtime_requirement: None,
            }],
            actions: vec![DependencyEffectAction {
                path: vec!["errors".to_owned(), "Net".to_owned(), "request".to_owned()],
                effect_args: vec![EffectActionArgKind::StringPattern],
                selector_param_names: vec!["host".to_owned()],
                selector_defaults: vec![Some(etas_types::EffectArgRef::Wildcard)],
                returns_never: false,
                runtime_requirement: None,
            }],
            extensions: Vec::new(),
        };
        let registry = EffectRegistry::from_hir_types_and_dependencies(
            &HirProgram::default(),
            &etas_types::TypeOutput::default(),
            &metadata,
        )
        .expect("valid dependency metadata should build a registry");

        let canonical = registry
            .action_by_name("errors.Net.request")
            .expect("canonical dependency action is registered");
        let imported = registry
            .action_by_name("Net.request")
            .expect("import-visible dependency action is registered");
        assert_eq!(canonical, imported);
    }

    #[test]
    fn malformed_dependency_metadata_fails_closed() {
        let metadata = DependencyEffectMetadata {
            tags: vec![DependencyEffectTag {
                path: Vec::new(),
                runtime_requirement: None,
            }],
            actions: Vec::new(),
            extensions: Vec::new(),
        };
        let error = EffectRegistry::from_hir_types_and_dependencies(
            &HirProgram::default(),
            &etas_types::TypeOutput::default(),
            &metadata,
        )
        .expect_err("empty dependency effect paths must be rejected");
        assert!(matches!(
            error,
            EffectPipelineError::InvalidExternalMetadata { .. }
        ));
    }
}
