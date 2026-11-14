use std::collections::HashMap;

use etas_core::Diagnostic;

use crate::{
    HirBlockArena, HirExprArena, HirHandlerArmArena, HirItemArena, HirItemId, HirModuleArena,
    HirModuleId, HirPatArena, HirStmtArena, HirTypeArena, ScopeTree, SourceMap, SymbolTable,
};

#[derive(Clone, Debug, Default, etas_core::RetainedSize, serde::Serialize, serde::Deserialize)]
pub struct HirProgram {
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

    pub fn declaration_path(&self, symbol: crate::SymbolId) -> Option<Vec<String>> {
        let symbol = self.symbols.get(symbol)?;
        if let crate::SymbolDef::ImportAlias { path, .. } = &symbol.def {
            return Some(path.clone());
        }
        let owner = match self.agent_methods.get(&symbol.id) {
            Some(method) => self.symbols.get(method.agent)?,
            None => symbol,
        };
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
}

#[cfg(test)]
mod tests;
