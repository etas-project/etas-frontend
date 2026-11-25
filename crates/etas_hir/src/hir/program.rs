use std::collections::HashMap;

use etas_core::Diagnostic;

use crate::{
    HirBlockArena, HirExprArena, HirHandlerArmArena, HirItemArena, HirItemId, HirModuleArena,
    HirModuleId, HirPatArena, HirStmtArena, HirTypeArena, ScopeTree, SourceMap, SymbolTable,
};

#[derive(Clone, Debug, Default, etas_core::RetainedSize, serde::Serialize, serde::Deserialize)]
pub struct HirProgram {
    pub constructor_external_bindings: Vec<HirConstructorExternalBinding>,
    /// Explicit provider choice; source syntax must not fall back per symbol to
    /// the legacy collection representation or algorithm implementations.
    pub source_standard_types: bool,
    /// Definition-site expressions; the declared parameter type remains T.
    #[serde(default, serialize_with = "crate::serde_ordered::map")]
    pub parameter_defaults: HashMap<crate::SymbolId, crate::HirExprId>,
    pub modules: Vec<HirModuleId>,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub items: HirItemArena,
    #[serde(serialize_with = "crate::serde_ordered::map")]
    pub item_annotations: HashMap<HirItemId, Vec<crate::HirAnnotation>>,
    #[serde(serialize_with = "crate::serde_ordered::map")]
    pub agent_methods: HashMap<crate::SymbolId, HirAgentMethod>,
    #[serde(serialize_with = "crate::serde_ordered::map")]
    pub source_import_targets: HashMap<crate::SymbolId, crate::SymbolId>,
    #[serde(serialize_with = "crate::serde_ordered::map")]
    pub standard_action_targets: HashMap<crate::SymbolId, etas_std::StdSymbolId>,
    /// A lexical flow name may denote several checked input signatures.
    #[serde(default, serialize_with = "crate::serde_ordered::map")]
    pub flow_overloads: HashMap<crate::SymbolId, Vec<crate::SymbolId>>,
    pub exprs: HirExprArena,
    pub handler_arms: HirHandlerArmArena,
    pub stmts: HirStmtArena,
    pub pats: HirPatArena,
    pub types: HirTypeArena,
    pub blocks: HirBlockArena,
    pub modules_arena: HirModuleArena,
    pub source_map: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(serialize_with = "crate::serde_ordered::set")]
    optimized_out_blocks: std::collections::HashSet<crate::HirBlockId>,
    #[serde(serialize_with = "crate::serde_ordered::set")]
    optimized_out_exprs: std::collections::HashSet<crate::HirExprId>,
    #[serde(serialize_with = "crate::serde_ordered::set")]
    optimized_out_patterns: std::collections::HashSet<crate::HirPatId>,
    #[serde(serialize_with = "crate::serde_ordered::set")]
    optimized_out_statements: std::collections::HashSet<crate::HirStmtId>,
}

#[derive(Clone, Debug, etas_core::RetainedSize, serde::Serialize, serde::Deserialize)]
pub struct HirAgentMethod {
    pub agent: crate::SymbolId,
    pub symbol: crate::SymbolId,
    pub body: HirItemId,
    pub implementation: Option<HirItemId>,
}

impl HirProgram {
    /// Whether an arm discards every payload and cannot resume the request.
    /// Such an arm does not depend on the payload or result type of an overload.
    pub fn handler_ignores_request_abi<E>(
        &self,
        arm: &crate::HirHandlerArm,
        mut checkpoint: impl FnMut() -> Result<(), E>,
    ) -> Result<bool, E> {
        use crate::{HirPat, HirStmt, HirTreeChild};
        if arm.continuation.is_some()
            || !arm
                .patterns
                .iter()
                .all(|pat| matches!(self.pats[*pat], HirPat::Wildcard { .. }))
        {
            return Ok(false);
        }
        let mut pending = vec![HirTreeChild::Block(arm.body)];
        while let Some(node) = pending.pop() {
            checkpoint()?;
            match node {
                HirTreeChild::Block(block) => {
                    let block = &self.blocks[block];
                    pending.extend(block.stmts.iter().copied().map(HirTreeChild::Stmt));
                    pending.extend(block.final_expr.map(HirTreeChild::Expr));
                }
                HirTreeChild::Stmt(stmt) => {
                    if matches!(self.stmts[stmt], HirStmt::Resume { .. }) {
                        return Ok(false);
                    }
                    crate::for_each_stmt_child(self, stmt, |node| pending.push(node));
                }
                HirTreeChild::Expr(expr) => {
                    crate::for_each_expr_child(self, expr, |node| pending.push(node));
                }
                HirTreeChild::HandlerArm(handler) => {
                    pending.push(HirTreeChild::Block(self.handler_arms[handler].body));
                }
                HirTreeChild::Pattern(_) => (),
            }
        }
        Ok(true)
    }

    pub fn flow_overload_group(&self, symbol: crate::SymbolId) -> Option<&[crate::SymbolId]> {
        let direct = |symbol| {
            self.flow_overloads
                .get(&symbol)
                .or_else(|| {
                    self.flow_overloads
                        .values()
                        .find(|group| group.contains(&symbol))
                })
                .map(Vec::as_slice)
        };
        direct(symbol).or_else(|| direct(self.canonical_source_symbol(symbol)?))
    }

    pub fn flow_overload_root(&self, symbols: &[crate::SymbolId]) -> Option<crate::SymbolId> {
        let group = self.flow_overload_group(*symbols.first()?)?;
        symbols
            .iter()
            .all(|symbol| group.contains(symbol))
            .then(|| group[0])
    }

    /// The authored declaration's parameter list, including its receiver when present.
    pub fn parameter_list(&self, parameter: crate::SymbolId) -> Option<&[crate::SymbolId]> {
        let crate::SymbolDef::Param { owner, .. } = self.symbols.get(parameter)?.def else {
            return None;
        };
        match self.items.get(owner)? {
            crate::HirItem::Flow(flow) => Some(&flow.params),
            crate::HirItem::Tool(tool) => Some(&tool.params),
            crate::HirItem::Agent(agent) => Some(&agent.params),
            crate::HirItem::Effect(effect) => effect
                .body
                .actions()
                .iter()
                .find(|action| action.params.contains(&parameter))
                .map(|action| action.params.as_slice()),
            crate::HirItem::Spec(spec) => spec.items.iter().find_map(|member| match member {
                crate::HirSpecItem::FlowSignature(signature)
                    if signature.params.contains(&parameter) =>
                {
                    Some(signature.params.as_slice())
                }
                _ => None,
            }),
            crate::HirItem::Impl(implementation) => {
                implementation.items.iter().find_map(|member| match member {
                    crate::HirImplItem::Flow(flow) if flow.params.contains(&parameter) => {
                        Some(flow.params.as_slice())
                    }
                    crate::HirImplItem::Action(action) if action.params.contains(&parameter) => {
                        Some(action.params.as_slice())
                    }
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Parameters visible to this parameter's definition-site default expression.
    pub fn preceding_parameters(&self, parameter: crate::SymbolId) -> Option<&[crate::SymbolId]> {
        let parameters = self.parameter_list(parameter)?;
        let index = parameters.iter().position(|symbol| *symbol == parameter)?;
        Some(&parameters[..index])
    }

    pub fn mark_block_optimized_out(&mut self, block: crate::HirBlockId) {
        self.optimized_out_blocks.insert(block);
    }

    pub fn mark_expr_optimized_out(&mut self, expression: crate::HirExprId) {
        self.optimized_out_exprs.insert(expression);
    }

    pub fn mark_pattern_optimized_out(&mut self, pattern: crate::HirPatId) {
        self.optimized_out_patterns.insert(pattern);
    }

    pub fn mark_statement_optimized_out(&mut self, statement: crate::HirStmtId) {
        self.optimized_out_statements.insert(statement);
    }

    pub fn is_block_optimized_out(&self, block: crate::HirBlockId) -> bool {
        self.optimized_out_blocks.contains(&block)
    }

    pub fn is_expr_optimized_out(&self, expression: crate::HirExprId) -> bool {
        self.optimized_out_exprs.contains(&expression)
    }

    pub fn is_pattern_optimized_out(&self, pattern: crate::HirPatId) -> bool {
        self.optimized_out_patterns.contains(&pattern)
    }

    pub fn is_statement_optimized_out(&self, statement: crate::HirStmtId) -> bool {
        self.optimized_out_statements.contains(&statement)
    }

    /// Follow resolver-owned source bindings without reinterpreting textual paths.
    pub fn canonical_source_symbol(&self, symbol: crate::SymbolId) -> Option<crate::SymbolId> {
        let mut current = symbol;
        let mut visited = std::collections::HashSet::new();
        loop {
            let symbol = self.symbols.get(current)?;
            let Some(target) = self.source_import_targets.get(&current) else {
                return Some(current);
            };
            if !matches!(symbol.def, crate::SymbolDef::ImportAlias { .. })
                || !visited.insert(current)
            {
                return None;
            }
            current = *target;
        }
    }

    pub fn item_module(&self, item: HirItemId) -> Option<HirModuleId> {
        let symbol = self.items.get(item)?.symbol()?;
        Some(self.symbols.get(symbol)?.defining_module)
    }

    pub fn agent_method_is_inherent(&self, method: &HirAgentMethod) -> bool {
        method.implementation.is_none_or(|item| matches!(self.items.get(item), Some(crate::HirItem::Impl(decl))
            if matches!(&decl.target, crate::HirImplTarget::Agent { specs, .. } if specs.is_empty())))
    }

    pub fn agent_method(&self, agent: crate::SymbolId, name: &str) -> Option<&HirAgentMethod> {
        let candidates = self
            .agent_methods
            .values()
            .filter(|method| {
                method.agent == agent
                    && self
                        .symbols
                        .get(method.symbol)
                        .is_some_and(|symbol| symbol.name == name)
            })
            .collect::<Vec<_>>();
        let inherent = candidates
            .iter()
            .copied()
            .filter(|method| self.agent_method_is_inherent(method))
            .collect::<Vec<_>>();
        match if inherent.is_empty() {
            candidates.as_slice()
        } else {
            inherent.as_slice()
        } {
            [method] => Some(*method),
            _ => None,
        }
    }

    pub fn enclosing_impl(&self, scope: crate::ScopeId) -> Option<&crate::HirImplDecl> {
        let parent = self.scopes.get(self.scopes.get(scope)?.parent?)?;
        let crate::ScopeOwner::Item(owner) = parent.owner else {
            return None;
        };
        match self.items.get(owner)? {
            crate::HirItem::Impl(decl) => Some(decl),
            _ => None,
        }
    }

    pub fn impl_target_symbol(
        &self,
        implementation: &crate::HirImplDecl,
    ) -> Option<crate::SymbolId> {
        let path = match &implementation.target {
            crate::HirImplTarget::Inherent { target, .. } => target,
            crate::HirImplTarget::SpecSatisfaction { self_type, .. } => {
                let crate::HirType::Path { path, .. } = self.types.get(*self_type)? else {
                    return None;
                };
                path
            }
            _ => return None,
        };
        let crate::ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        self.canonical_source_symbol(symbol)
    }

    pub fn active_agent_item(&self, symbol: crate::SymbolId) -> Option<HirItemId> {
        let crate::SymbolDef::Item { item } = self.symbols.get(symbol)?.def else {
            return None;
        };
        matches!(self.items.get(item), Some(crate::HirItem::Agent(agent)) if agent.state.is_some())
            .then_some(item)
    }

    pub fn active_method_agent(&self, item: HirItemId) -> Option<HirItemId> {
        let crate::HirItem::Flow(flow) = self.items.get(item)? else {
            return None;
        };
        let owner = self.impl_target_symbol(self.enclosing_impl(flow.scope)?)?;
        self.active_agent_item(owner)
    }

    pub fn member_tool_owner(&self, item: HirItemId) -> Option<crate::SymbolId> {
        let crate::HirItem::Tool(tool) = self.items.get(item)? else {
            return None;
        };
        self.impl_target_symbol(self.enclosing_impl(tool.scope)?)
    }

    pub fn member_tool_receiver(&self, item: HirItemId) -> Option<crate::SymbolId> {
        let crate::HirItem::Tool(tool) = self.items.get(item)? else {
            return None;
        };
        self.member_tool_owner(item)?;
        let first = *tool.params.first()?;
        (self.symbols.get(first)?.name == "self").then_some(first)
    }

    /// A method's tool declaration overrides its Agent's tool declaration.
    pub fn agent_tools_annotation(
        &self,
        agent: HirItemId,
        method: HirItemId,
    ) -> Option<&crate::HirAnnotation> {
        [method, agent].into_iter().find_map(|item| {
            self.item_annotations.get(&item)?.iter().rev().find(|annotation| {
                matches!(annotation.path.segments.as_slice(), [name] if name.name == "tools")
            })
        })
    }

    /// Include the tool list inherited by a method when collecting executable dependencies.
    pub fn execution_annotations(
        &self,
        item: HirItemId,
    ) -> impl Iterator<Item = &crate::HirAnnotation> {
        let own = self.item_annotations.get(&item).into_iter().flatten();
        let inherited = self
            .active_method_agent(item)
            .or_else(|| {
                let symbol = self.items.get(item)?.symbol()?;
                let method = self.agent_methods.get(&symbol)?;
                match self.symbols.get(method.agent)?.def {
                    crate::SymbolDef::Item { item } => Some(item),
                    _ => None,
                }
            })
            .filter(|agent| *agent != item && self.agent_tools_annotation(item, item).is_none())
            .and_then(|agent| self.agent_tools_annotation(agent, item));
        own.chain(inherited)
    }

    pub fn declaration_path(&self, symbol: crate::SymbolId) -> Option<Vec<String>> {
        let symbol = self.symbols.get(symbol)?;
        if let crate::SymbolDef::ImportAlias { path, .. } = &symbol.def {
            return Some(path.clone());
        }
        let owner = self
            .agent_methods
            .get(&symbol.id)
            .map(|method| method.agent)
            .or_else(|| match symbol.def {
                crate::SymbolDef::Item { item } => self.member_tool_owner(item),
                _ => None,
            })
            .and_then(|owner| self.symbols.get(owner))
            .unwrap_or(symbol);
        let mut path = self
            .modules_arena
            .get(owner.defining_module)?
            .name
            .as_ref()
            .map(|module| {
                module
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        path.push(owner.name.clone());
        if owner.id != symbol.id {
            path.push(symbol.name.clone());
        }
        Some(path)
    }
    /// The body synthesized for a trusted native declaration has no concrete
    /// caller scope. Its binding provenance survives in the checked image.
    pub fn native_binding_owner(&self, call: crate::HirExprId) -> Option<&crate::HirFlowDecl> {
        let crate::HirExpr::Call { callee, .. } = self.exprs.get(call)? else {
            return None;
        };
        let crate::HirExpr::Path(path) = self.exprs.get(*callee)? else {
            return None;
        };
        let crate::ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.symbols.get(symbol)?;
        if !matches!(
            symbol.def,
            crate::SymbolDef::ImportAlias {
                origin: crate::ImportAliasOrigin::NativeBinding,
                ..
            }
        ) {
            return None;
        }
        let crate::HirItem::Flow(flow) = self.items.get(symbol.defining_item?)? else {
            return None;
        };
        Some(flow)
    }
}

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, etas_core::RetainedSize, serde::Serialize, serde::Deserialize)]
pub struct HirConstructorExternalBinding {
    pub symbol: crate::SymbolId,
    pub package: u32,
    pub path: Vec<String>,
    pub span: etas_core::Span,
}
