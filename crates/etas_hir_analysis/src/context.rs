use etas_hir::{
    HirBlockId, HirBodyRef, HirExprId, HirHandlerArmId, HirItemId, HirProgram, HirTreeIndex,
    HirTreeIndexError, HirTreeView,
};

#[derive(Clone, Debug)]
pub struct HirAnalysisContext {
    tree_index: HirTreeIndex,
}

impl HirAnalysisContext {
    pub fn try_new(hir: &HirProgram) -> Result<Self, HirTreeIndexError> {
        Ok(Self {
            tree_index: HirTreeIndex::try_build(hir)?,
        })
    }

    pub fn new(hir: &HirProgram) -> Self {
        Self::try_new(hir).expect("HIR tree invariants should hold for analysis")
    }

    pub fn tree_index(&self) -> &HirTreeIndex {
        &self.tree_index
    }

    pub fn view<'hir>(&self, hir: &'hir HirProgram) -> HirTreeView<'hir> {
        HirTreeView::from_validated_index(hir, self.tree_index.clone())
    }

    pub fn item_primary_body(&self, item: HirItemId) -> Option<HirBodyRef> {
        self.tree_index.item_body.get(&item).copied()
    }

    pub fn body_root_expr(&self, body: HirBodyRef) -> Option<HirExprId> {
        self.tree_index
            .body_exprs
            .get(&body)
            .and_then(|exprs| exprs.first())
            .copied()
    }

    pub fn body_block(&self, body: HirBodyRef) -> Option<HirBlockId> {
        self.tree_index
            .body_blocks
            .get(&body)
            .and_then(|blocks| blocks.first())
            .copied()
    }

    pub fn handler_arm_body(&self, hir: &HirProgram, arm: HirHandlerArmId) -> Option<HirBlockId> {
        hir.handler_arms.get(arm).map(|arm| arm.body)
    }
}
