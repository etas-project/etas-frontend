//! Installed execution units are slices of one checked program. Source text and
//! declaration/type interfaces remain shared; no source is rewritten or solved.
use super::LibraryHirImage;
use crate::project::SemanticDependency;
use etas_hir::*;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct LibrarySlicePlan {
    pub modules: BTreeSet<Vec<String>>,
    pub definitions: BTreeSet<HirItemId>,
}

#[derive(Default)]
struct Nodes {
    expressions: BTreeSet<HirExprId>,
    blocks: BTreeSet<HirBlockId>,
    statements: BTreeSet<HirStmtId>,
    patterns: BTreeSet<HirPatId>,
    arms: BTreeSet<HirHandlerArmId>,
}
impl<'v, 'h> HirVisitor<'v, 'h> for Nodes {
    fn enter_expr(&mut self, node: ExprView<'v, 'h>) {
        self.expressions.insert(node.id());
    }
    fn enter_block(&mut self, node: BlockView<'v, 'h>) {
        self.blocks.insert(node.id());
    }
    fn enter_stmt(&mut self, node: StmtView<'v, 'h>) {
        self.statements.insert(node.id());
    }
    fn enter_pattern(&mut self, node: PatternView<'v, 'h>) {
        self.patterns.insert(node.id());
    }
    fn enter_handler_arm(&mut self, node: HandlerArmView<'v, 'h>) {
        self.arms.insert(node.id());
    }
}
impl LibraryHirImage {
    pub fn execution_slices(&self, checked: &crate::CheckedProgramData) -> Vec<LibrarySlicePlan> {
        let mut unique = BTreeSet::new();
        self.program
            .items
            .iter()
            .filter(|(_, item)| {
                matches!(
                    item,
                    HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_)
                )
            })
            .filter_map(|(root, _)| {
                let closure = checked
                    .semantic_dependencies
                    .closure([SemanticDependency::Definition(root)]);
                let definitions = closure
                    .iter()
                    .filter_map(|node| match node {
                        SemanticDependency::Definition(item) => Some(*item),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                if !unique.insert(definitions.clone()) {
                    return None;
                }
                let modules = definitions
                    .iter()
                    .map(|item| {
                        let source = self.program.items[*item].span().source;
                        self.module_index.modules[self.module_index.by_source[&source]]
                            .path
                            .segments
                            .clone()
                    })
                    .collect();
                Some(LibrarySlicePlan {
                    modules,
                    definitions,
                })
            })
            .collect()
    }

    pub fn execution_slice(&self, definitions: &BTreeSet<HirItemId>) -> Self {
        let revision =
            HirRevision::from_shared_program(self.program.clone()).expect("checked library HIR");
        let view = revision.view();
        let mut nodes = Nodes::default();
        for (item, declaration) in self.program.items.iter() {
            if definitions.contains(&item)
                || !matches!(
                    declaration,
                    HirItem::Flow(_)
                        | HirItem::Tool(_)
                        | HirItem::Agent(_)
                        | HirItem::TopLevelLet(_)
                        | HirItem::Impl(_)
                )
            {
                walk_item(&view, item, &mut nodes);
            }
        }
        // Attributes and default expressions belong to the shared declarations.
        for expression in self.program.parameter_defaults.values().copied().chain(
            self.program
                .item_annotations
                .values()
                .flatten()
                .flat_map(|annotation| &annotation.args)
                .map(|arg| match arg {
                    HirAnnotationArg::Positional { value, .. }
                    | HirAnnotationArg::Named { value, .. } => *value,
                }),
        ) {
            walk_expr_tree(&view, expression, &mut nodes);
        }
        let mut image = self.clone();
        let mut program = self.program.as_ref().clone();
        for (item, declaration) in program.items.iter_mut() {
            if definitions.contains(&item) {
                continue;
            }
            match declaration {
                HirItem::Flow(flow) if flow.body.as_block().is_some() => {
                    flow.body = HirFlowBody::Decl { span: flow.span }
                }
                HirItem::Tool(tool) => tool.body = HirToolBody::Decl { span: tool.span },
                HirItem::Agent(agent) => agent.body = HirAgentBody::Decl { span: agent.span },
                HirItem::TopLevelLet(value) => value.value = None,
                _ => {}
            }
        }
        program
            .exprs
            .retain(|id, _| nodes.expressions.contains(&id));
        program.blocks.retain(|id, _| nodes.blocks.contains(&id));
        program.stmts.retain(|id, _| nodes.statements.contains(&id));
        program.pats.retain(|id, _| nodes.patterns.contains(&id));
        program
            .handler_arms
            .retain(|id, _| nodes.arms.contains(&id));
        let mut scopes = program
            .scopes
            .iter()
            .filter(|scope| match scope.owner {
                ScopeOwner::Module(_) | ScopeOwner::Item(_) => true,
                ScopeOwner::Block(block) => nodes.blocks.contains(&block),
                ScopeOwner::Lambda(expression)
                | ScopeOwner::Handler(expression)
                | ScopeOwner::MatchArm(expression) => nodes.expressions.contains(&expression),
            })
            .map(|scope| scope.id)
            .collect::<BTreeSet<_>>();
        let mut pending = scopes.iter().copied().collect::<Vec<_>>();
        while let Some(scope) = pending.pop() {
            if let Some(parent) = program.scopes.get(scope).and_then(|scope| scope.parent)
                && scopes.insert(parent)
            {
                pending.push(parent);
            }
        }
        program.scopes.retain(|scope| scopes.contains(&scope.id));
        let sources = &mut program.source_map;
        sources
            .expr_sources
            .retain(|id, _| nodes.expressions.contains(id));
        sources
            .block_sources
            .retain(|id, _| nodes.blocks.contains(id));
        sources
            .stmt_sources
            .retain(|id, _| nodes.statements.contains(id));
        sources
            .pat_sources
            .retain(|id, _| nodes.patterns.contains(id));
        sources
            .handler_arm_sources
            .retain(|id, _| nodes.arms.contains(id));
        for references in sources.syntax_to_hir.values_mut() {
            references.retain(|node| match node {
                HirNodeRef::Item(_) => true,
                HirNodeRef::Expr(id) => nodes.expressions.contains(id),
                HirNodeRef::Block(id) => nodes.blocks.contains(id),
                HirNodeRef::Stmt(id) => nodes.statements.contains(id),
                HirNodeRef::Pat(id) => nodes.patterns.contains(id),
                HirNodeRef::HandlerArm(id) => nodes.arms.contains(id),
                _ => true,
            });
        }
        sources.syntax_to_hir.retain(|_, nodes| !nodes.is_empty());
        let mut syntax = sources
            .syntax_to_hir
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut pending = syntax.iter().copied().collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            if let Some(parent) = sources.syntax_nodes.get(&node).and_then(|node| node.parent)
                && syntax.insert(parent)
            {
                pending.push(parent);
            }
        }
        sources.syntax_nodes.retain(|id, _| syntax.contains(id));
        image
            .body_slots
            .retain(|item, _| definitions.contains(item));
        image
            .reachability
            .analysis_items
            .retain(|item| definitions.contains(item));
        image
            .reachability
            .reachable_items
            .retain(|item| definitions.contains(item));
        let facts = Arc::make_mut(&mut image.types);
        facts
            .declared_call_arguments
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .existential_packs
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .checked_method_calls
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .bound_method_values
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .agent_method_calls
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .source_flow_calls
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .library_flow_calls
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .std_intrinsic_calls
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .action_calls
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .expr_types
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .reflections
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .definition_operations
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .agent_inferences
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .agent_controllers
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .task_owners
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .expr_memory_places
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .field_projections
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .index_facts
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .slice_facts
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .checked_index_errors
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .generic_instantiations
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .try_facts
            .retain(|id, _| nodes.expressions.contains(id));
        facts
            .trusted_literals
            .retain(|id| nodes.expressions.contains(id));
        facts
            .stmt_types
            .retain(|id, _| nodes.statements.contains(id));
        facts
            .pattern_types
            .retain(|id, _| nodes.patterns.contains(id));
        facts
            .handler_actions
            .retain(|id, _| nodes.arms.contains(id));
        facts
            .deferred_handler_instantiations
            .retain(|id, _| nodes.arms.contains(id));
        facts
            .body_type_dependencies
            .retain(|item, _| definitions.contains(item));
        facts
            .body_spec_dependencies
            .retain(|item, _| definitions.contains(item));
        let effects = Arc::make_mut(&mut image.effects);
        effects
            .handled_body_requests
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .native_call_targets
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .source_call_arguments
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .dynamic_call_sources
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .iteration_call_sources
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .metadata_call_targets
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .generic_effect_rows
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .expr_effects
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .handler_values
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .handle_applications
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .performed_actions
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .try_captures
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .latent_effects
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .std_callable_values
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .std_action_calls
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .task_calls
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .continuation_transfers
            .retain(|id, _| nodes.expressions.contains(id));
        effects
            .stmt_effects
            .retain(|id, _| nodes.statements.contains(id));
        effects
            .handler_action_effects
            .retain(|id, _| nodes.arms.contains(id));
        let keep_unit = |unit: &etas_effects::EffectUnit| match unit {
            etas_effects::EffectUnit::Item(item) => definitions.contains(item),
            etas_effects::EffectUnit::ParameterDefault { expr, .. } => {
                nodes.expressions.contains(expr)
            }
            etas_effects::EffectUnit::HandlerArm { arm, .. } => nodes.arms.contains(arm),
            etas_effects::EffectUnit::AnonymousFlow { value, .. } => {
                nodes.expressions.contains(value)
            }
        };
        effects.unit_effects.retain(|unit, _| keep_unit(unit));
        effects.suspension.retain(|unit, _| keep_unit(unit));
        // Use the same checked owners as BodyEffectOutput::project. Captures
        // and environments are body facts, not shared declaration interfaces.
        effects
            .parameter_projections
            .retain(|_, fact| definitions.contains(&fact.owner));
        effects
            .callable_environments
            .retain(|_, fact| definitions.contains(&fact.owner));
        effects
            .captured_callables
            .retain(|id, _| effects.callable_environments.contains_key(&id.environment));
        effects.metadata_callables.retain(|source, _| {
            source
                .environment
                .is_none_or(|id| effects.callable_environments.contains_key(&id))
        });
        effects
            .callable_references
            .retain(|reference, _| match reference.environment {
                Some(id) => effects.callable_environments.contains_key(&id),
                None => definitions.contains(&reference.owner),
            });
        effects
            .frozen_callable_graphs
            .retain(|_, fact| definitions.contains(&fact.owner));
        effects
            .item_effects
            .retain(|item, _| definitions.contains(item));
        effects
            .public_contracts
            .retain(|contract| definitions.contains(&contract.item));
        effects
            .requirements
            .items
            .retain(|item, _| definitions.contains(item));
        effects
            .interpreter_support
            .items
            .retain(|item, _| definitions.contains(item));
        let owned_symbol = |symbol: &SymbolId| {
            program
                .symbols
                .get(*symbol)
                .and_then(|symbol| match &symbol.def {
                    SymbolDef::Param { owner, .. } => Some(*owner),
                    _ => symbol.defining_item,
                })
                .is_some_and(|item| definitions.contains(&item))
        };
        effects
            .symbol_effects
            .retain(|symbol, _| owned_symbol(symbol));
        effects
            .requirements
            .symbols
            .retain(|symbol, _| owned_symbol(symbol));
        effects
            .inferred_parameter_effects
            .retain(|symbol, _| owned_symbol(symbol));
        let closure = self.semantic_dependencies.closure(
            definitions
                .iter()
                .copied()
                .map(SemanticDependency::Definition),
        );
        Arc::make_mut(&mut image.semantic_dependencies)
            .edges
            .retain(|node, _| closure.contains(node));
        image.program = Arc::new(program);
        image
    }
}
