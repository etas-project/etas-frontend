use std::collections::HashMap;

use etas_hir::{HirBlockId, HirExprId, HirItemId};

use super::module_index::{AstBodyRef, AstItemRef};

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct HirItemBindings {
    pub ast_to_hir: HashMap<AstItemRef, HirItemId>,
    pub hir_to_ast: HashMap<HirItemId, AstItemRef>,
    pub external_import_members: HashMap<etas_hir::SymbolId, HirExternalImportMember>,
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct HirExternalImportMember {
    pub package: crate::ExternalPackageId,
    pub path: Vec<String>,
    pub span: etas_core::Span,
}

impl HirItemBindings {
    pub fn body_item(&self, body: &AstBodyRef, hir: &etas_hir::HirProgram) -> Option<HirItemId> {
        let item = *self.ast_to_hir.get(&body.item)?;
        match body.impl_member {
            None => Some(item),
            Some(member) => {
                let etas_hir::HirItem::Impl(decl) = hir.items.get(item)? else {
                    return None;
                };
                match decl.items.get(member)? {
                    etas_hir::HirImplItem::Callable { item } => Some(*item),
                    _ => None,
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct HirBodyBindings {
    pub body_items: HashMap<AstBodyRef, HirItemId>,
    pub ast_to_root_block: HashMap<AstBodyRef, HirBlockId>,
    pub root_block_to_ast: HashMap<HirBlockId, AstBodyRef>,
    pub body_blocks: HashMap<AstBodyRef, Vec<HirBlockId>>,
    pub block_to_ast: HashMap<HirBlockId, AstBodyRef>,
    pub body_exprs: HashMap<AstBodyRef, Vec<HirExprId>>,
    pub expr_to_ast: HashMap<HirExprId, AstBodyRef>,
}
