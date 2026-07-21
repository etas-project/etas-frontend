use etas_hir::{HirItem, HirProgram, HirToolBody};
use etas_hir_analysis::{HirAnalysisContext, interprocedural::HirAnalysisBody};

use super::{EffectAnonymousFlowBody, EffectUnit};

impl EffectUnit {
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
            EffectUnit::Item(item) => item_body(hir, context, item),
            EffectUnit::HandlerArm { arm, .. } => HirAnalysisBody::HandlerArm(arm),
            EffectUnit::AnonymousFlow { body, .. } => match body {
                EffectAnonymousFlowBody::Expr(expr) => HirAnalysisBody::Expr(expr),
                EffectAnonymousFlowBody::Block(block) => HirAnalysisBody::Block(block),
            },
            EffectUnit::FirstClassFlowCall { call, .. } => HirAnalysisBody::Expr(call),
        }
    }
}

fn item_body(
    hir: &HirProgram,
    context: &HirAnalysisContext,
    item: etas_hir::HirItemId,
) -> HirAnalysisBody {
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
            HirToolBody::Source(_) => HirAnalysisBody::Missing,
            HirToolBody::Decl { .. } => HirAnalysisBody::External,
            HirToolBody::Error { .. } => HirAnalysisBody::Missing,
        },
        Some(HirItem::Error { .. }) | None => HirAnalysisBody::Missing,
        Some(
            HirItem::Type(_)
            | HirItem::TypeAlias(_)
            | HirItem::Enum(_)
            | HirItem::Spec(_)
            | HirItem::Flow(_)
            | HirItem::Agent(_)
            | HirItem::TopLevelLet(_)
            | HirItem::Impl(_)
            | HirItem::Effect(_)
            | HirItem::Protocol(_),
        ) => HirAnalysisBody::External,
    }
}
