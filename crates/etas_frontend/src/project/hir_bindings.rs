use std::collections::HashMap;

use etas_hir::{HirBlockId, HirExprId, HirItemId};

use super::module_index::{AstBodyRef, AstItemRef};

#[derive(Clone, Debug, Default)]
pub struct HirItemBindings {
    pub ast_to_hir: HashMap<AstItemRef, HirItemId>,
    pub hir_to_ast: HashMap<HirItemId, AstItemRef>,
}

#[derive(Clone, Debug, Default)]
pub struct HirBodyBindings {
    pub ast_to_root_block: HashMap<AstBodyRef, HirBlockId>,
    pub root_block_to_ast: HashMap<HirBlockId, AstBodyRef>,
    pub body_blocks: HashMap<AstBodyRef, Vec<HirBlockId>>,
    pub block_to_ast: HashMap<HirBlockId, AstBodyRef>,
    pub body_exprs: HashMap<AstBodyRef, Vec<HirExprId>>,
    pub expr_to_ast: HashMap<HirExprId, AstBodyRef>,
}
