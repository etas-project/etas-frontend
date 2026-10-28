use super::program::{LowerCtx, SyntaxKey};
use crate::{HirBlockId, HirFlowBody, HirItemId, ScopeId};
use etas_core::Span;
use etas_syntax::{RelocatedBodySyntax, SourceBodyView, ast};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyOverlayError {
    ConflictingLexicalSource(Span),
    MissingCallable(Span),
    DuplicateCallable(Span),
    UnappliedCallable(Span),
}

pub(super) fn key(span: Span) -> SyntaxKey {
    SyntaxKey {
        source: span.source,
        start: span.range.start.to_usize() as u32,
        end: span.range.end.to_usize() as u32,
    }
}

pub(super) fn callable_slots(program: &ast::Program, slots: &mut HashSet<SyntaxKey>) {
    for item in &program.items {
        match &item.item {
            ast::Item::Flow(flow) => {
                slots.insert(key(flow.span));
            }
            ast::Item::Agent(agent) if matches!(agent.body, ast::AgentBody::Source(_)) => {
                slots.insert(key(agent.span));
            }
            ast::Item::Tool(tool) if matches!(tool.body, ast::ToolBody::Source(_)) => {
                slots.insert(key(tool.span));
            }
            ast::Item::Impl(implementation) => {
                for member in &implementation.items {
                    if let ast::ImplItem::Flow(flow) = member {
                        slots.insert(key(flow.span));
                    }
                }
            }
            _ => {}
        }
    }
}

impl LowerCtx {
    pub(super) fn take_body_overlay(&mut self, span: Span) -> Option<Arc<RelocatedBodySyntax>> {
        self.body_overlays.remove(&key(span))
    }

    pub(super) fn lower_overlay_block(
        &mut self,
        body: SourceBodyView<'_>,
        scope: ScopeId,
        item: HirItemId,
    ) -> HirBlockId {
        match body {
            SourceBodyView::Block(block) => self.lower_block(block, scope, item),
            SourceBodyView::Expression(expr) => self.lower_expr_body_block(expr, scope, item),
        }
    }

    pub(super) fn lower_overlay_flow(
        &mut self,
        body: &RelocatedBodySyntax,
        scope: ScopeId,
        item: HirItemId,
    ) -> HirFlowBody {
        let block = self.lower_overlay_block(body.body(), scope, item);
        let lowered = HirFlowBody::Block(block);
        match body.trailing_handler() {
            Some(handler) => self.wrap_flow_body_with_handler(lowered, handler, scope, item),
            None => lowered,
        }
    }
}
