//! Canonical checked objects: shared declaration/type data and disjoint module
//! bodies. Execution closures are assembled views of these objects, never
//! independently published copies of a checked program.
use super::{LibraryHirImage, LibraryHirImageError, decode_prepared_value, error};
use crate::{ModuleId, SemanticDependency};
use etas_hir::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryObjectIndex {
    pub compiler_version: String,
    pub dependencies: Arc<crate::project::CheckedSemanticDependencies>,
    pub declarations: BTreeMap<etas_code::DefinitionKey, HirItemId>,
    pub methods: BTreeMap<etas_code::MethodId, HirItemId>,
    pub definitions: BTreeMap<HirItemId, ModuleId>,
    pub modules: BTreeMap<ModuleId, Vec<String>>,
}
impl LibraryObjectIndex {
    pub fn encode(&self) -> Result<Vec<u8>, LibraryHirImageError> {
        let mut bytes = b"ETASOBJ1".to_vec();
        bytes.extend(
            bincode::serde::encode_to_vec(self, bincode::config::standard()).map_err(error)?,
        );
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, LibraryHirImageError> {
        let payload = bytes
            .strip_prefix(b"ETASOBJ1")
            .ok_or_else(|| error("incompatible checked object index"))?;
        let index: Self = decode_prepared_value(payload)?;
        if index.compiler_version != crate::FRONTEND_COMPILER_VERSION {
            return Err(error("incompatible checked object compiler"));
        }
        Ok(index)
    }
    pub fn closure(&self, roots: impl IntoIterator<Item = HirItemId>) -> BTreeSet<HirItemId> {
        self.dependencies
            .closure(roots.into_iter().map(SemanticDependency::Definition))
            .into_iter()
            .filter_map(|node| match node {
                SemanticDependency::Definition(item) => Some(item),
                _ => None,
            })
            .collect()
    }
    pub fn callable(&self, callable: &etas_code::CallableId) -> Option<HirItemId> {
        callable
            .as_declaration()
            .and_then(|key| self.declarations.get(key))
            .or_else(|| callable.as_method().and_then(|key| self.methods.get(key)))
            .copied()
    }
    pub fn selected_modules(&self, definitions: &BTreeSet<HirItemId>) -> BTreeSet<ModuleId> {
        definitions
            .iter()
            .map(|item| self.definitions[item])
            .collect()
    }
}

pub struct LibraryObjectArchive {
    pub index: LibraryObjectIndex,
    pub shared: Vec<u8>,
    pub modules: BTreeMap<ModuleId, Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Body {
    Flow(HirFlowBody),
    Tool(HirToolBody),
    Agent(HirAgentBody),
    Value(Option<HirExprId>),
}
#[derive(Default, Serialize, Deserialize)]
struct ModuleBodies {
    bodies: BTreeMap<HirItemId, Body>,
    exprs: Vec<(HirExprId, HirExpr)>,
    blocks: Vec<(HirBlockId, HirBlock)>,
    stmts: Vec<(HirStmtId, HirStmt)>,
    pats: Vec<(HirPatId, HirPat)>,
    arms: Vec<(HirHandlerArmId, HirHandlerArm)>,
    scopes: Vec<Scope>,
    source_map: SourceMap,
    types: etas_types::TypeFacts,
    effects: etas_effects::EffectFacts,
}

fn owner(index: &HirTreeIndex, node: HirOwner) -> Option<HirItemId> {
    match node {
        HirOwner::Item(item) => Some(item),
        HirOwner::Body(body) => Some(body.item),
        HirOwner::Expr(expr) => index.expr_item.get(&expr).copied(),
        HirOwner::Stmt(stmt) => index.stmt_item.get(&stmt).copied(),
        HirOwner::Block(block) => index
            .block_owner
            .get(&block)
            .and_then(|owner_| owner(index, *owner_)),
        HirOwner::HandlerArm(arm) => index
            .handler_arm_owner
            .get(&arm)
            .and_then(|owner_| owner(index, *owner_)),
        HirOwner::Pattern(pat) => index
            .pat_owner
            .get(&pat)
            .and_then(|owner_| owner(index, *owner_)),
        HirOwner::Module(_) => None,
    }
}
fn unit_owner(unit: &etas_effects::EffectUnit) -> HirItemId {
    match unit {
        etas_effects::EffectUnit::Item(item) => *item,
        etas_effects::EffectUnit::ParameterDefault { owner, .. }
        | etas_effects::EffectUnit::AnonymousFlow { owner, .. }
        | etas_effects::EffectUnit::HandlerArm { owner, .. } => *owner,
    }
}

macro_rules! type_expressions {
    ($apply:ident) => {
        $apply!(
            declared_call_arguments,
            existential_packs,
            checked_method_calls,
            bound_method_values,
            agent_method_calls,
            source_flow_calls,
            library_flow_calls,
            std_intrinsic_calls,
            action_calls,
            expr_types,
            reflections,
            definition_operations,
            agent_inferences,
            agent_controllers,
            task_owners,
            expr_memory_places,
            field_projections,
            index_facts,
            slice_facts,
            checked_index_errors,
            generic_instantiations,
            try_facts
        );
    };
}
macro_rules! effect_expressions {
    ($apply:ident) => {
        $apply!(
            handled_body_requests,
            native_call_targets,
            source_call_arguments,
            dynamic_call_sources,
            iteration_call_sources,
            metadata_call_targets,
            generic_effect_rows,
            expr_effects,
            handler_values,
            handle_applications,
            performed_actions,
            try_captures,
            latent_effects,
            std_callable_values,
            std_action_calls,
            task_calls,
            continuation_transfers
        );
    };
}

impl LibraryHirImage {
    pub fn checked_objects(&self) -> Result<LibraryObjectArchive, LibraryHirImageError> {
        let revision = HirRevision::from_shared_program(self.program.clone())
            .map_err(|e| error(format!("{e:?}")))?;
        let tree = revision.index();
        let mut common = self.execution_slice(&BTreeSet::new());
        // Declaration coordinates and immutable HIR type/symbol tables are
        // shared once. The object loader never parses the retained source text.
        common.body_slots = self.body_slots.clone();
        common.reachability = self.reachability.clone();
        common.semantic_dependencies = Arc::default();
        common.declarations.clear();
        common.methods.clear();
        common.types = self.types.clone();
        common.effects = self.effects.clone();
        let mut program = common.program.as_ref().clone();
        let definitions = self
            .program
            .items
            .iter()
            .map(|(id, item)| (id, self.module_index.by_source[&item.span().source]))
            .collect::<BTreeMap<_, _>>();
        let index = LibraryObjectIndex {
            compiler_version: self.compiler_version.clone(),
            dependencies: self.semantic_dependencies.clone(),
            declarations: self.declarations.clone(),
            methods: self.methods.clone(),
            definitions,
            modules: self
                .module_index
                .modules
                .iter()
                .map(|(id, module)| (id, module.path.segments.clone()))
                .collect(),
        };
        let mut modules = index
            .modules
            .keys()
            .map(|id| (*id, ModuleBodies::default()))
            .collect::<BTreeMap<_, _>>();
        let module = |item: HirItemId| index.definitions[&item];
        for (id, item) in self.program.items.iter() {
            let body = match item {
                HirItem::Flow(flow) if flow.body.as_block().is_some() => {
                    Some(Body::Flow(flow.body))
                }
                HirItem::Tool(tool) => Some(Body::Tool(tool.body.clone())),
                HirItem::Agent(agent) => Some(Body::Agent(agent.body.clone())),
                HirItem::TopLevelLet(value) => Some(Body::Value(value.value)),
                _ => None,
            };
            if let Some(body) = body {
                modules
                    .get_mut(&module(id))
                    .unwrap()
                    .bodies
                    .insert(id, body);
            }
        }
        macro_rules! nodes {
            ($field:ident, $target:ident, $node:ident) => {
                for (id, value) in self.program.$field.iter() {
                    if program.$field.get(id).is_none() {
                        match owner(tree, HirOwner::$node(id)) {
                            Some(item) => modules
                                .get_mut(&module(item))
                                .unwrap()
                                .$target
                                .push((id, value.clone())),
                            None => program.$field.extend_linked([(id, value.clone())]),
                        }
                    }
                }
            };
        }
        nodes!(exprs, exprs, Expr);
        nodes!(blocks, blocks, Block);
        nodes!(stmts, stmts, Stmt);
        nodes!(pats, pats, Pattern);
        nodes!(handler_arms, arms, HandlerArm);
        for scope in self.program.scopes.iter() {
            if program.scopes.get(scope.id).is_none() {
                let item = match scope.owner {
                    ScopeOwner::Block(block) => owner(tree, HirOwner::Block(block)),
                    ScopeOwner::Lambda(expr)
                    | ScopeOwner::Handler(expr)
                    | ScopeOwner::MatchArm(expr) => owner(tree, HirOwner::Expr(expr)),
                    ScopeOwner::Item(item) => Some(item),
                    ScopeOwner::Module(_) => None,
                };
                match item {
                    Some(item) => modules
                        .get_mut(&module(item))
                        .unwrap()
                        .scopes
                        .push(scope.clone()),
                    None => program.scopes.extend_linked([scope.clone()]),
                }
            }
        }
        macro_rules! source_nodes {
            ($field:ident, $arena:ident, $node:ident) => {
                for (id, origin) in &self.program.source_map.$field {
                    if !program.source_map.$field.contains_key(id) {
                        match owner(tree, HirOwner::$node(*id)) {
                            Some(item) => {
                                modules
                                    .get_mut(&module(item))
                                    .unwrap()
                                    .source_map
                                    .$field
                                    .insert(*id, origin.clone());
                            }
                            None => {
                                program.source_map.$field.insert(*id, origin.clone());
                            }
                        }
                    }
                }
            };
        }
        source_nodes!(expr_sources, exprs, Expr);
        source_nodes!(block_sources, blocks, Block);
        source_nodes!(stmt_sources, stmts, Stmt);
        source_nodes!(pat_sources, pats, Pattern);
        source_nodes!(handler_arm_sources, handler_arms, HandlerArm);
        for (id, node) in &self.program.source_map.syntax_nodes {
            if !program.source_map.syntax_nodes.contains_key(id) {
                if let Some(module) = self.module_index.by_source.get(&node.span.source) {
                    modules
                        .get_mut(module)
                        .unwrap()
                        .source_map
                        .syntax_nodes
                        .insert(*id, *node);
                } else {
                    program.source_map.syntax_nodes.insert(*id, *node);
                }
            }
        }
        for (id, references) in &self.program.source_map.syntax_to_hir {
            for reference in references {
                if program
                    .source_map
                    .syntax_to_hir
                    .get(id)
                    .is_some_and(|refs| refs.contains(reference))
                {
                    continue;
                }
                let owning = match reference {
                    HirNodeRef::Expr(id) => owner(tree, HirOwner::Expr(*id)),
                    HirNodeRef::Block(id) => owner(tree, HirOwner::Block(*id)),
                    HirNodeRef::Stmt(id) => owner(tree, HirOwner::Stmt(*id)),
                    HirNodeRef::Pat(id) => owner(tree, HirOwner::Pattern(*id)),
                    HirNodeRef::HandlerArm(id) => owner(tree, HirOwner::HandlerArm(*id)),
                    _ => None,
                };
                let map = match owning {
                    Some(item) => &mut modules.get_mut(&module(item)).unwrap().source_map,
                    None => &mut program.source_map,
                };
                map.syntax_to_hir.entry(*id).or_default().push(*reference);
            }
        }
        let types = Arc::make_mut(&mut common.types);
        macro_rules! type_exprs { ($($field:ident),*) => {$(
            for (id, value) in std::mem::take(&mut types.$field) {
                if program.exprs.get(id).is_none() && let Some(item) = tree.expr_item.get(&id) {
                    modules.get_mut(&module(*item)).unwrap().types.$field.insert(id, value);
                } else { types.$field.insert(id, value); }
            }
        )*}; }
        type_expressions!(type_exprs);
        macro_rules! type_nodes {
            ($field:ident, $arena:ident, $kind:ident) => {
                for (id, value) in std::mem::take(&mut types.$field) {
                    if program.$arena.get(id).is_none()
                        && let Some(item) = owner(tree, HirOwner::$kind(id))
                    {
                        modules
                            .get_mut(&module(item))
                            .unwrap()
                            .types
                            .$field
                            .insert(id, value);
                    } else {
                        types.$field.insert(id, value);
                    }
                }
            };
        }
        type_nodes!(stmt_types, stmts, Stmt);
        type_nodes!(pattern_types, pats, Pattern);
        type_nodes!(handler_actions, handler_arms, HandlerArm);
        type_nodes!(deferred_handler_instantiations, handler_arms, HandlerArm);
        for id in std::mem::take(&mut types.trusted_literals) {
            if program.exprs.get(id).is_none()
                && let Some(item) = tree.expr_item.get(&id)
            {
                modules
                    .get_mut(&module(*item))
                    .unwrap()
                    .types
                    .trusted_literals
                    .insert(id);
            } else {
                types.trusted_literals.insert(id);
            }
        }
        macro_rules! type_items { ($($field:ident),*) => {$(
            for (id, value) in std::mem::take(&mut types.$field) { modules.get_mut(&module(id)).unwrap().types.$field.insert(id, value); }
        )*}; }
        type_items!(body_type_dependencies, body_spec_dependencies);
        let effects = Arc::make_mut(&mut common.effects);
        macro_rules! effect_exprs { ($($field:ident),*) => {$(
            for (id, value) in std::mem::take(&mut effects.$field) {
                if program.exprs.get(id).is_none() && let Some(item) = tree.expr_item.get(&id) {
                    modules.get_mut(&module(*item)).unwrap().effects.$field.insert(id, value);
                } else { effects.$field.insert(id, value); }
            }
        )*}; }
        effect_expressions!(effect_exprs);
        macro_rules! effect_nodes {
            ($field:ident, $arena:ident, $kind:ident) => {
                for (id, value) in std::mem::take(&mut effects.$field) {
                    if program.$arena.get(id).is_none()
                        && let Some(item) = owner(tree, HirOwner::$kind(id))
                    {
                        modules
                            .get_mut(&module(item))
                            .unwrap()
                            .effects
                            .$field
                            .insert(id, value);
                    } else {
                        effects.$field.insert(id, value);
                    }
                }
            };
        }
        effect_nodes!(stmt_effects, stmts, Stmt);
        effect_nodes!(handler_action_effects, handler_arms, HandlerArm);
        for (id, value) in std::mem::take(&mut effects.item_effects) {
            modules
                .get_mut(&module(id))
                .unwrap()
                .effects
                .item_effects
                .insert(id, value);
        }
        for (id, value) in std::mem::take(&mut effects.unit_effects) {
            modules
                .get_mut(&module(unit_owner(&id)))
                .unwrap()
                .effects
                .unit_effects
                .insert(id, value);
        }
        for (id, value) in std::mem::take(&mut effects.suspension) {
            modules
                .get_mut(&module(unit_owner(&id)))
                .unwrap()
                .effects
                .suspension
                .insert(id, value);
        }
        for (id, value) in std::mem::take(&mut effects.parameter_projections) {
            modules
                .get_mut(&module(value.owner))
                .unwrap()
                .effects
                .parameter_projections
                .insert(id, value);
        }
        for (id, value) in std::mem::take(&mut effects.frozen_callable_graphs) {
            modules
                .get_mut(&module(value.owner))
                .unwrap()
                .effects
                .frozen_callable_graphs
                .insert(id, value);
        }
        // Resolve captured values through their checked environment ownership.
        let environments = std::mem::take(&mut effects.callable_environments);
        for (id, value) in std::mem::take(&mut effects.captured_callables) {
            modules
                .get_mut(&module(environments[&id.environment].owner))
                .unwrap()
                .effects
                .captured_callables
                .insert(id, value);
        }
        for (id, value) in std::mem::take(&mut effects.callable_references) {
            let item = id
                .environment
                .map_or(id.owner, |id| environments[&id].owner);
            modules
                .get_mut(&module(item))
                .unwrap()
                .effects
                .callable_references
                .insert(id, value);
        }
        for (id, value) in std::mem::take(&mut effects.metadata_callables) {
            if let Some(environment) = id.environment {
                modules
                    .get_mut(&module(environments[&environment].owner))
                    .unwrap()
                    .effects
                    .metadata_callables
                    .insert(id, value);
            } else {
                effects.metadata_callables.insert(id, value);
            }
        }
        for (id, value) in environments {
            modules
                .get_mut(&module(value.owner))
                .unwrap()
                .effects
                .callable_environments
                .insert(id, value);
        }
        for value in std::mem::take(&mut effects.public_contracts) {
            modules
                .get_mut(&module(value.item))
                .unwrap()
                .effects
                .public_contracts
                .push(value);
        }
        common.program = Arc::new(program);
        Ok(LibraryObjectArchive {
            shared: common.encode_prepared()?,
            index,
            modules: modules
                .into_iter()
                .map(|(id, body)| {
                    Ok((
                        id,
                        bincode::serde::encode_to_vec(body, bincode::config::standard())
                            .map_err(error)?,
                    ))
                })
                .collect::<Result<_, LibraryHirImageError>>()?,
        })
    }

    pub fn from_checked_objects(
        shared: &[u8],
        index: &LibraryObjectIndex,
        modules: impl IntoIterator<Item = Vec<u8>>,
        definitions: &BTreeSet<HirItemId>,
    ) -> Result<Self, LibraryHirImageError> {
        let mut image = Self::decode_prepared(shared)?;
        image.semantic_dependencies = index.dependencies.clone();
        image.declarations = index.declarations.clone();
        image.methods = index.methods.clone();
        let program = Arc::make_mut(&mut image.program);
        let types = Arc::make_mut(&mut image.types);
        let effects = Arc::make_mut(&mut image.effects);
        for payload in modules {
            let body: ModuleBodies = decode_prepared_value(&payload)?;
            for (id, value) in body.bodies {
                match (&mut program.items[id], value) {
                    (HirItem::Flow(flow), Body::Flow(body)) => flow.body = body,
                    (HirItem::Tool(tool), Body::Tool(body)) => tool.body = body,
                    (HirItem::Agent(agent), Body::Agent(body)) => agent.body = body,
                    (HirItem::TopLevelLet(value), Body::Value(body)) => value.value = body,
                    _ => unreachable!("checked body declaration kind"),
                }
            }
            program.exprs.extend_linked(body.exprs);
            program.blocks.extend_linked(body.blocks);
            program.stmts.extend_linked(body.stmts);
            program.pats.extend_linked(body.pats);
            program.handler_arms.extend_linked(body.arms);
            program.scopes.extend_linked(body.scopes);
            program
                .source_map
                .expr_sources
                .extend(body.source_map.expr_sources);
            program
                .source_map
                .block_sources
                .extend(body.source_map.block_sources);
            program
                .source_map
                .stmt_sources
                .extend(body.source_map.stmt_sources);
            program
                .source_map
                .pat_sources
                .extend(body.source_map.pat_sources);
            program
                .source_map
                .handler_arm_sources
                .extend(body.source_map.handler_arm_sources);
            program
                .source_map
                .syntax_nodes
                .extend(body.source_map.syntax_nodes);
            for (id, refs) in body.source_map.syntax_to_hir {
                program
                    .source_map
                    .syntax_to_hir
                    .entry(id)
                    .or_default()
                    .extend(refs);
            }
            macro_rules! type_exprs { ($($field:ident),*) => {$(types.$field.extend(body.types.$field);)*}; }
            type_expressions!(type_exprs);
            types.stmt_types.extend(body.types.stmt_types);
            types.pattern_types.extend(body.types.pattern_types);
            types.handler_actions.extend(body.types.handler_actions);
            types
                .deferred_handler_instantiations
                .extend(body.types.deferred_handler_instantiations);
            types
                .body_type_dependencies
                .extend(body.types.body_type_dependencies);
            types
                .body_spec_dependencies
                .extend(body.types.body_spec_dependencies);
            types.trusted_literals.extend(body.types.trusted_literals);
            macro_rules! effect_exprs { ($($field:ident),*) => {$(effects.$field.extend(body.effects.$field);)*}; }
            effect_expressions!(effect_exprs);
            effects.stmt_effects.extend(body.effects.stmt_effects);
            effects
                .handler_action_effects
                .extend(body.effects.handler_action_effects);
            effects.item_effects.extend(body.effects.item_effects);
            effects.unit_effects.extend(body.effects.unit_effects);
            effects.suspension.extend(body.effects.suspension);
            effects
                .parameter_projections
                .extend(body.effects.parameter_projections);
            effects
                .frozen_callable_graphs
                .extend(body.effects.frozen_callable_graphs);
            effects
                .captured_callables
                .extend(body.effects.captured_callables);
            effects
                .callable_references
                .extend(body.effects.callable_references);
            effects
                .metadata_callables
                .extend(body.effects.metadata_callables);
            effects
                .callable_environments
                .extend(body.effects.callable_environments);
            effects
                .public_contracts
                .extend(body.effects.public_contracts);
        }
        Ok(image.execution_slice(definitions))
    }
}
