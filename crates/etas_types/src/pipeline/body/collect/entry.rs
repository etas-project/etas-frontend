use etas_hir::{HirAgentBody, HirItem, HirToolBody};

use crate::{
    ItemSignature, Type,
    pipeline::{
        body::{collect::block::collect_block, state::BodyPipelineState},
        context::{BodyCollectContext, TypePipelineContext},
    },
};

pub fn collect_body_item(ctx: &mut TypePipelineContext<'_>, state: &mut BodyPipelineState) {
    let item = ctx.hir.items[state.item].clone();
    match item {
        HirItem::Flow(flow) => {
            state.expected_return = ctx
                .signature_facts
                .item_signatures
                .get(&state.item)
                .and_then(|signature| match signature {
                    ItemSignature::Flow(signature) => Some(signature.output),
                    _ => None,
                });
            let mut body = BodyCollectContext { ctx, state };
            let expected = body.state.expected_return;
            collect_block(&mut body, flow.body.block(), expected);
        }
        HirItem::Tool(tool) => {
            if let HirToolBody::Source(body_decl) = tool.body {
                state.expected_return = ctx
                    .signature_facts
                    .item_signatures
                    .get(&state.item)
                    .and_then(|signature| match signature {
                        ItemSignature::Tool(signature) => Some(signature.output),
                        _ => None,
                    });
                let mut body = BodyCollectContext { ctx, state };
                let expected = body.state.expected_return;
                collect_block(&mut body, body_decl.block(), expected);
            }
        }
        HirItem::Agent(agent) => {
            let HirAgentBody::Source { block } = agent.body else {
                return;
            };
            state.expected_return = Some(ctx.interner.intern(Type::Prompt));
            let mut body = BodyCollectContext { ctx, state };
            let expected = body.state.expected_return;
            collect_block(&mut body, block, expected);
        }
        HirItem::TopLevelLet(top) => {
            state.expected_return = top
                .type_annotation
                .and_then(|ty| crate::lower::type_ref::lower_type_ref(ctx, ty))
                .or_else(|| {
                    ctx.signature_facts
                        .item_signatures
                        .get(&state.item)
                        .and_then(|signature| match signature {
                            ItemSignature::TopLevelLet(crate::TopLevelLetSignature { ty }) => {
                                Some(*ty)
                            }
                            _ => None,
                        })
                });
            let mut body = BodyCollectContext { ctx, state };
            let expected = body.state.expected_return;
            let ty =
                crate::pipeline::body::collect::expr::collect_expr(&mut body, top.value, expected);
            let ty = body.state.expected_return.unwrap_or(ty);
            body.state.provisional.item_signatures.insert(
                body.state.item,
                ItemSignature::TopLevelLet(crate::TopLevelLetSignature { ty }),
            );
            body.state.provisional.symbol_types.insert(
                top.symbol,
                crate::SymbolTypeFact::TopLevelLet {
                    ty,
                    classification: top.classification,
                },
            );
        }
        _ => {}
    }
}
