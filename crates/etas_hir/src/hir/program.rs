use std::collections::HashMap;

use etas_core::Diagnostic;

use crate::{
    HirBlockArena, HirExprArena, HirHandlerArmArena, HirItemArena, HirItemId, HirModuleArena,
    HirModuleId, HirPatArena, HirStmtArena, HirTypeArena, ScopeTree, SourceMap, SymbolTable,
};

#[derive(Clone, Debug, Default)]
pub struct HirProgram {
    pub modules: Vec<HirModuleId>,
    pub symbols: SymbolTable,
    pub scopes: ScopeTree,
    pub items: HirItemArena,
    pub item_annotations: HashMap<HirItemId, Vec<crate::HirAnnotation>>,
    pub exprs: HirExprArena,
    pub handler_arms: HirHandlerArmArena,
    pub stmts: HirStmtArena,
    pub pats: HirPatArena,
    pub types: HirTypeArena,
    pub blocks: HirBlockArena,
    pub modules_arena: HirModuleArena,
    pub source_map: SourceMap,
    pub diagnostics: Vec<Diagnostic>,
}
