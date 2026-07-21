use etas_hir::{HirBlockId, HirExprId, HirHandlerArmId, HirItem, HirItemId, HirProgram};

use crate::{HirAnalysisContext, interprocedural::HirAnalysisBody};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HirSemanticUnit {
    Item(HirItemId),
    AnonymousFlow {
        owner: HirItemId,
        expr: HirExprId,
        body: HirAnonymousFlowBody,
    },
    HandlerArm(HirHandlerArmId),
    TopLevelLet(HirItemId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HirAnonymousFlowBody {
    Expr(HirExprId),
    Block(HirBlockId),
}

impl HirSemanticUnit {
    pub fn owner(self) -> Option<HirItemId> {
        match self {
            Self::Item(item) | Self::TopLevelLet(item) => Some(item),
            Self::AnonymousFlow { owner, .. } => Some(owner),
            Self::HandlerArm(_) => None,
        }
    }

    pub fn analysis_body(self, hir: &HirProgram) -> HirAnalysisBody {
        let context = HirAnalysisContext::new(hir);
        self.analysis_body_with_context(hir, &context)
    }

    pub fn analysis_body_with_context(
        self,
        hir: &HirProgram,
        context: &HirAnalysisContext,
    ) -> HirAnalysisBody {
        match self {
            Self::Item(item) => item_body(hir, context, item),
            Self::TopLevelLet(item) => match hir.items.get(item) {
                Some(HirItem::TopLevelLet(decl)) => HirAnalysisBody::Expr(decl.value),
                Some(_) => HirAnalysisBody::Missing,
                None => HirAnalysisBody::Missing,
            },
            Self::AnonymousFlow { body, .. } => match body {
                HirAnonymousFlowBody::Expr(expr) => HirAnalysisBody::Expr(expr),
                HirAnonymousFlowBody::Block(block) => HirAnalysisBody::Block(block),
            },
            Self::HandlerArm(arm) => {
                if hir.handler_arms.get(arm).is_some() {
                    HirAnalysisBody::HandlerArm(arm)
                } else {
                    HirAnalysisBody::Missing
                }
            }
        }
    }
}

fn item_body(hir: &HirProgram, context: &HirAnalysisContext, item: HirItemId) -> HirAnalysisBody {
    if let Some(body) = context.item_primary_body(item) {
        if let Some(expr) = context.body_root_expr(body) {
            return HirAnalysisBody::Expr(expr);
        }
        if let Some(block) = context.body_block(body) {
            return HirAnalysisBody::Block(block);
        }
    }

    match hir.items.get(item) {
        Some(HirItem::Tool(tool)) => match tool.body {
            etas_hir::HirToolBody::Source(_) => HirAnalysisBody::Missing,
            etas_hir::HirToolBody::Decl { .. } => HirAnalysisBody::External,
            etas_hir::HirToolBody::Error { .. } => HirAnalysisBody::Missing,
        },
        Some(HirItem::Error { .. }) | None => HirAnalysisBody::Missing,
        Some(HirItem::TopLevelLet(_)) => HirAnalysisBody::Missing,
        Some(_) => HirAnalysisBody::External,
    }
}
