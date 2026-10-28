use etas_core::Span;
use etas_hir::{HirExprId, SymbolId};

use crate::{
    ConstraintOrigin, SpecObligation, SymbolTypeFact, TypeConstraint, TypeId,
    pipeline::context::BodyCollectContext,
};

pub(super) fn collect<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    symbol: SymbolId,
    span: Span,
    expected: Option<TypeId>,
) -> Option<TypeId> {
    let fact = ctx
        .state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))?
        .clone();
    if !matches!(&fact, SymbolTypeFact::Flow { signature } | SymbolTypeFact::Tool { signature } if !signature.generic_params.is_empty())
    {
        return None;
    }
    let candidate = super::expr::callable_candidate_from_fact(ctx, fact, true)?;
    for param in &candidate.generic_params {
        if param.kind != crate::CallableGenericParamKind::Type {
            continue;
        }
        for bound in &param.bounds {
            ctx.state.spec_obligations.push(SpecObligation {
                ty: param.subject,
                spec: bound.spec.clone(),
                args: bound.args.clone(),
                span,
            });
        }
    }
    let lexical_effect_params = match ctx.ctx.item_signature(ctx.state.item) {
        Some(crate::ItemSignature::Flow(signature) | crate::ItemSignature::Tool(signature)) => {
            signature.generic_params.as_slice()
        }
        Some(crate::ItemSignature::Agent(signature)) => signature
            .callable()
            .map_or(&[][..], |signature| signature.generic_params.as_slice()),
        _ => &[],
    }
    .iter()
    .filter(|parameter| parameter.kind == crate::CallableGenericParamKind::Effect)
    .map(|parameter| parameter.name.clone())
    .collect();
    ctx.emit(TypeConstraint::CallableReference {
        expr,
        signature: candidate.ty,
        generic_params: candidate.generic_params,
        expected,
        lexical_effect_params,
        origin: ConstraintOrigin { span },
    });
    Some(candidate.ty)
}
