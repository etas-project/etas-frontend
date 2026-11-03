use etas_hir::{
    HirDeclarationConformance, HirDeclarationConformanceTarget, HirEffectArg, HirItem, HirSpecExpr,
};

use crate::{lower::type_ref::lower_type_ref, pipeline::context::TypePipelineContext};

/// Trace selector types are signature facts, not executable body expressions.
pub fn collect_trace_pattern_types(ctx: &mut TypePipelineContext<'_>) {
    let mut pending = Vec::new();
    for (_, item) in ctx.hir.items.iter() {
        match item {
            HirItem::Spec(spec) => pending.extend(spec.trace.iter()),
            HirItem::Flow(flow) => add_inline(&mut pending, &flow.conformances),
            HirItem::Tool(tool) => add_inline(&mut pending, &tool.conformances),
            HirItem::Agent(agent) => add_inline(&mut pending, &agent.conformances),
            _ => {}
        }
    }
    let mut types = Vec::new();
    while let Some(expr) = pending.pop() {
        match expr {
            HirSpecExpr::Atom(effect)
            | HirSpecExpr::Allow {
                pattern: effect, ..
            }
            | HirSpecExpr::Deny {
                pattern: effect, ..
            } => {
                for arg in &effect.args {
                    match arg {
                        HirEffectArg::Type(ty) => types.push(*ty),
                        HirEffectArg::PatternBinding { bounds, .. } => {
                            types
                                .extend(bounds.iter().flat_map(|bound| bound.args.iter().copied()));
                        }
                        _ => {}
                    }
                }
            }
            HirSpecExpr::And { lhs, rhs, .. } | HirSpecExpr::Or { lhs, rhs, .. } => {
                pending.push(lhs);
                pending.push(rhs);
            }
            HirSpecExpr::Before { before, after, .. }
            | HirSpecExpr::After { before, after, .. } => {
                pending.push(before);
                pending.push(after);
            }
        }
    }
    for ty in types {
        lower_type_ref(ctx, ty);
    }
}

fn add_inline<'a>(
    pending: &mut Vec<&'a HirSpecExpr>,
    conformances: &'a [HirDeclarationConformance],
) {
    pending.extend(
        conformances
            .iter()
            .filter_map(|conformance| match &conformance.target {
                HirDeclarationConformanceTarget::InlineTraceSpec(expr) => Some(expr),
                _ => None,
            }),
    );
}
