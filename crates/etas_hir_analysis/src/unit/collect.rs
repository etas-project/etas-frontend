use etas_hir::{HirBodyKind, HirItem, HirProgram, HirToolBody};

use crate::HirAnalysisContext;

use super::{HirAnonymousFlowBody, HirSemanticUnit};

pub struct HirSemanticUnitCollector;

impl HirSemanticUnitCollector {
    pub fn collect(hir: &HirProgram) -> Vec<HirSemanticUnit> {
        let context = HirAnalysisContext::new(hir);
        Self::collect_with_context(hir, &context)
    }

    pub fn collect_with_context(
        hir: &HirProgram,
        context: &HirAnalysisContext,
    ) -> Vec<HirSemanticUnit> {
        let view = context.view(hir);
        let mut units = Vec::new();

        for module in view.modules() {
            for item in module.items() {
                match item.data() {
                    HirItem::Flow(_) | HirItem::Agent(_) => {
                        units.push(HirSemanticUnit::Item(item.id()));
                    }
                    HirItem::Tool(tool) => {
                        units.push(HirSemanticUnit::Item(item.id()));
                        if !matches!(tool.body, HirToolBody::Source(_)) {
                            continue;
                        }
                    }
                    HirItem::TopLevelLet(_) => {
                        units.push(HirSemanticUnit::TopLevelLet(item.id()));
                    }
                    _ => {}
                }

                for body in item.bodies() {
                    match body.id().kind {
                        HirBodyKind::Lambda(expr) => {
                            if let Some(body) = anonymous_flow_body(body) {
                                units.push(HirSemanticUnit::AnonymousFlow {
                                    owner: item.id(),
                                    expr,
                                    body,
                                });
                            }
                        }
                        HirBodyKind::HandlerArm(arm) => {
                            units.push(HirSemanticUnit::HandlerArm(arm));
                        }
                        _ => {}
                    }
                }
            }
        }

        units.sort();
        units.dedup();
        units
    }
}

fn anonymous_flow_body(body: etas_hir::BodyView<'_, '_>) -> Option<HirAnonymousFlowBody> {
    if let Some(expr) = body.root_exprs().next() {
        return Some(HirAnonymousFlowBody::Expr(expr.id()));
    }
    body.blocks()
        .next()
        .map(|block| HirAnonymousFlowBody::Block(block.id()))
}
