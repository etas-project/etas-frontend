use etas_hir::{HirImplItem, HirImplTarget, HirItem};

use crate::{
    facts::TypeImplMethodFact,
    lower::type_ref::{lower_path_type, lower_type_ref},
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_type_impl_methods(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let implementations = ctx
        .hir
        .items
        .iter()
        .filter_map(|(item, declaration)| match declaration {
            HirItem::Impl(decl)
                if !matches!(
                    decl.target,
                    HirImplTarget::Agent { .. } | HirImplTarget::Error
                ) =>
            {
                Some((item, decl.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (implementation, declaration) in implementations {
        let methods = declaration
            .items
            .iter()
            .filter_map(|method| {
                let HirImplItem::Callable { item } = method else {
                    return None;
                };
                let Some(HirItem::Flow(flow)) = ctx.hir.items.get(*item) else {
                    return None;
                };
                let name = ctx
                    .hir
                    .symbols
                    .get(flow.symbol)?
                    .name
                    .rsplit('.')
                    .next()?
                    .to_owned();
                Some((*item, flow.symbol, name))
            })
            .collect::<Vec<_>>();
        if methods.is_empty() {
            continue;
        }
        let (self_type, inherent) = match &declaration.target {
            HirImplTarget::SpecSatisfaction { self_type, .. } => {
                let Some(ty) = lower_type_ref(ctx, *self_type) else {
                    continue;
                };
                (ty, false)
            }
            HirImplTarget::Inherent { target, type_args } => {
                (lower_path_type(ctx, target, type_args, target.span), true)
            }
            HirImplTarget::Agent { .. } | HirImplTarget::Error => continue,
        };
        for (body, symbol, name) in methods {
            state.type_impl_methods.insert(
                symbol,
                TypeImplMethodFact {
                    symbol,
                    name,
                    implementation,
                    body,
                    self_type,
                    inherent,
                },
            );
        }
    }
}
