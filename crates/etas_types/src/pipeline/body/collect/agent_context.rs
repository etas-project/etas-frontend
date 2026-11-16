use etas_core::Span;
use etas_hir::{HirGenericArg, ResolveResult};

use crate::{
    CallableCandidate, CallableCandidateOperation, ConstraintOrigin, TypeConstraint, TypeId,
    ValidationRequest, pipeline::context::BodyCollectContext,
};

pub(super) fn current<E>(ctx: &BodyCollectContext<'_, '_, E>) -> Option<TypeId> {
    ctx.state.agent_contexts.last().copied().or_else(|| {
        ctx.ctx
            .signature_base
            .map_or(&ctx.ctx.signature_facts, |base| &base.facts)
            .agent_methods
            .values()
            .find(|method| method.body == ctx.state.item)
            .map(|method| method.context_type)
    })
}

pub(super) fn constrain<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    candidate: &CallableCandidate,
    span: Span,
) {
    let CallableCandidateOperation::StdIntrinsic { intrinsic, .. } = candidate.operation else {
        return;
    };
    if !matches!(
        intrinsic,
        etas_std::intrinsic::runtime::AGENT_STEP | etas_std::intrinsic::runtime::TOOL_INVOKE
    ) {
        return;
    }
    let Some(parameter) = candidate.signature.generic_params.first() else {
        return;
    };
    let active = current(ctx);
    if let Some(active) = active {
        ctx.emit(TypeConstraint::Equal {
            lhs: parameter.subject,
            rhs: active,
            origin: ConstraintOrigin { span },
        });
    }
    ctx.validate(ValidationRequest::AgentContext {
        ty: parameter.subject,
        active,
        span,
    });
}

pub(super) fn handler_context<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arm: &etas_hir::HirHandlerArm,
) -> Option<TypeId> {
    let ResolveResult::Resolved(symbol) = arm.action.action_symbol else {
        return None;
    };
    let super::handler_request::ActionIdentity::Standard(target) =
        super::handler_request::action_identity(ctx, symbol)
    else {
        return None;
    };
    let Some(etas_std::StdDecl::EffectAction(action)) = ctx
        .ctx
        .std_registry
        .symbol(target)
        .map(|symbol| &symbol.decl)
    else {
        return None;
    };
    if !matches!(
        action.compiler_role,
        Some(
            etas_std::CompilerActionRole::AgentInference | etas_std::CompilerActionRole::AgentStep
        )
    ) {
        return None;
    }
    let HirGenericArg::Type(ty) = arm.generic_args.first()? else {
        return None;
    };
    crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty)
}
