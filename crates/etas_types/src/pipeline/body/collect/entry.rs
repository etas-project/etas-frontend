use etas_hir::{HirAgentBody, HirItem, HirToolBody};

use crate::{
    ItemSignature,
    pipeline::{
        body::{collect::block::collect_block, state::BodyPipelineState},
        context::{BodyCollectContext, TypePipelineContext},
    },
};

pub fn collect_body_item<E>(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    checkpoint()?;
    let item = ctx.hir.items[state.item].clone();
    match item {
        HirItem::Flow(flow) => {
            state.expected_return =
                ctx.item_signature(state.item)
                    .and_then(|signature| match signature {
                        ItemSignature::Flow(signature) => Some(signature.output),
                        _ => None,
                    });
            let mut body = BodyCollectContext {
                ctx,
                state,
                execution: checkpoint,
            };
            let expected = body.state.expected_return;
            if flow.return_type.is_none() {
                body.state.inferred_returns = Some(Vec::new());
            }
            let hint = body
                .state
                .inferred_returns
                .is_none()
                .then_some(expected)
                .flatten();
            let result = collect_block(&mut body, flow.body.block(), hint)?;
            let final_expr = body.ctx.hir.blocks[flow.body.block()].final_expr;
            if let Some(mut candidates) = body.state.inferred_returns.take() {
                if let Some(expr) = final_expr {
                    candidates.push((
                        result,
                        crate::ConstraintOrigin {
                            span: body.ctx.hir.exprs[expr].span(&body.ctx.hir.blocks),
                        },
                    ));
                }
                if let Some(output) = expected {
                    let empty = body.primitive(crate::PrimitiveType::Unit);
                    body.emit(crate::TypeConstraint::ReturnType {
                        output,
                        candidates,
                        empty,
                        origin: crate::ConstraintOrigin { span: flow.span },
                    });
                }
            } else if final_expr.is_some()
                && let Some(output) = expected
            {
                body.emit(crate::TypeConstraint::Assignable {
                    from: result,
                    to: output,
                    origin: crate::ConstraintOrigin { span: flow.span },
                    reason: crate::AssignabilityReason::Return,
                });
            }
        }
        HirItem::Tool(tool) => {
            if let HirToolBody::Source(body_decl) = tool.body {
                state.expected_return =
                    ctx.item_signature(state.item)
                        .and_then(|signature| match signature {
                            ItemSignature::Tool(signature) => Some(signature.output),
                            _ => None,
                        });
                let mut body = BodyCollectContext {
                    ctx,
                    state,
                    execution: checkpoint,
                };
                let expected = body.state.expected_return;
                collect_block(&mut body, body_decl.block(), expected)?;
            }
        }
        HirItem::Agent(agent) => {
            let HirAgentBody::Source { block } = agent.body else {
                return Ok(());
            };
            state.expected_return =
                ctx.item_signature(state.item)
                    .and_then(|signature| match signature {
                        ItemSignature::Agent(signature) => {
                            signature.callable().map(|signature| signature.output)
                        }
                        _ => None,
                    });
            let mut body = BodyCollectContext {
                ctx,
                state,
                execution: checkpoint,
            };
            let expected = body.state.expected_return;
            collect_block(&mut body, block, expected)?;
        }
        HirItem::TopLevelLet(top) => {
            state.expected_return = top
                .type_annotation
                .and_then(|ty| crate::lower::type_ref::lower_type_ref(ctx, ty))
                .or_else(|| {
                    ctx.item_signature(state.item)
                        .and_then(|signature| match signature {
                            ItemSignature::TopLevelLet(crate::TopLevelLetSignature { ty }) => {
                                Some(*ty)
                            }
                            _ => None,
                        })
                });
            let mut body = BodyCollectContext {
                ctx,
                state,
                execution: checkpoint,
            };
            let expected = body.state.expected_return;
            let ty =
                crate::pipeline::body::collect::expr::collect_expr(&mut body, top.value, expected)?;
            if let Some(expected) = expected {
                body.emit(crate::TypeConstraint::Assignable {
                    from: ty,
                    to: expected,
                    origin: crate::ConstraintOrigin { span: top.span },
                    reason: crate::AssignabilityReason::Annotation,
                });
            }
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
    Ok(())
}
