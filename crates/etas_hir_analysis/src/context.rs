use etas_hir::{
    HirBlockId, HirBodyRef, HirExprId, HirHandlerArmId, HirItemId, HirProgram, HirRevision,
    HirTreeIndex, HirTreeView,
};

#[derive(Clone, Debug)]
pub struct HirAnalysisContext {
    revision: HirRevision,
}

impl HirAnalysisContext {
    pub fn new(revision: &HirRevision) -> Self {
        Self {
            revision: revision.clone(),
        }
    }

    pub fn program(&self) -> &HirProgram {
        self.revision.program()
    }

    pub fn revision(&self) -> &HirRevision {
        &self.revision
    }

    pub fn tree_index(&self) -> &HirTreeIndex {
        self.revision.index()
    }

    pub fn view(&self) -> HirTreeView<'_> {
        self.revision.view()
    }

    pub fn item_primary_body(&self, item: HirItemId) -> Option<HirBodyRef> {
        self.tree_index().item_body.get(&item).copied()
    }

    pub fn body_root_expr(&self, body: HirBodyRef) -> Option<HirExprId> {
        self.tree_index()
            .body_exprs
            .get(&body)
            .and_then(|exprs| exprs.first())
            .copied()
    }

    pub fn body_block(&self, body: HirBodyRef) -> Option<HirBlockId> {
        self.tree_index()
            .body_blocks
            .get(&body)
            .and_then(|blocks| blocks.first())
            .copied()
    }

    pub fn handler_arm_body(&self, arm: HirHandlerArmId) -> Option<HirBlockId> {
        self.program().handler_arms.get(arm).map(|arm| arm.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn context_keeps_its_revision_alive_and_never_rebuilds_an_index_for_queries() {
        let before = HirTreeIndex::thread_build_count();
        let revision = HirRevision::try_new(HirProgram::default()).unwrap();
        let program = Arc::downgrade(revision.shared_program());
        let context = HirAnalysisContext::new(&revision);
        let other = context.clone();
        assert!(std::ptr::eq(context.tree_index(), revision.index()));
        assert!(std::ptr::eq(context.program(), revision.program()));
        drop(revision);
        for _ in 0..128 {
            assert!(std::ptr::eq(context.view().index(), other.view().index()));
            assert!(std::ptr::eq(context.view().program(), context.program()));
        }
        assert_eq!(HirTreeIndex::thread_build_count() - before, 1);
        drop(context);
        assert!(program.upgrade().is_some());
        drop(other);
        assert!(program.upgrade().is_none());
    }
}
