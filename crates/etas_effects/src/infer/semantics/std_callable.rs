use etas_hir::{HirExpr, HirExprId, HirOwner, ResolveResult, SymbolDef};
use etas_std::{
    IntrinsicMemoryAccess, IntrinsicPurity, IntrinsicRuntimeRequirement, StdDecl, StdSymbolKind,
    TypeDeclKind,
};
use etas_types::{SymbolTypeFact, Type};

use super::engine::EffectSemantics;

impl EffectSemantics<'_> {
    pub(super) fn record_std_reference_result(
        &self,
        value: HirExprId,
        site: &etas_hir_analysis::interprocedural::CallSite<crate::EffectUnit>,
        state: &mut super::state::EffectState,
    ) -> Result<bool, super::specialize::EffectSpecializationError> {
        let invalid = |reason: &str| {
            super::specialize::EffectSpecializationError::InvalidCallableEnvironment {
                reason: reason.into(),
            }
        };
        if !self.expr_type_contains_flow(site.call) {
            return Ok(false);
        }
        let fact = self
            .inputs
            .std_callable_values
            .get(&value)
            .ok_or_else(|| invalid("standard callable result has no checked descriptor"))?;
        let Some(SymbolDef::ImportAlias { path, .. }) =
            self.hir.symbols.get(fact.symbol).map(|symbol| &symbol.def)
        else {
            return Err(invalid(
                "standard callable result has no resolved import identity",
            ));
        };
        let descriptor = self
            .std_registry
            .lookup_qualified(path)
            .and_then(|symbol| symbol.intrinsic.as_ref())
            .filter(|descriptor| descriptor.id.0 == fact.intrinsic)
            .ok_or_else(|| {
                invalid("standard callable result has an inconsistent intrinsic identity")
            })?;
        if descriptor.latent_effect != etas_std::IntrinsicLatentEffect::TransparentFirstArg {
            return Ok(false);
        }
        let Some(HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. }) =
            self.hir.exprs.get(site.call)
        else {
            return Err(invalid(
                "standard callable invocation has no checked argument list",
            ));
        };
        let argument = args
            .first()
            .and_then(super::shared::arg_expr)
            .ok_or_else(|| {
                invalid("transparent standard callable requires its checked payload argument")
            })?;
        let sources = self
            .latent_sources_for_expr(state, argument)
            .ok_or_else(|| invalid("standard callable payload has no checked latent origins"))?;
        self.record_callable_result_sources(site.call, sources, state)?;
        Ok(true)
    }

    pub(super) fn collect_std_callable_values(&mut self) {
        // Retained bodies also need these declaration facts, even when their
        // execution summaries come from a frozen contract.
        let hir = self.hir;
        for (expr, value) in hir.exprs.iter() {
            if matches!(value, HirExpr::Path(_)) {
                self.record_std_callable_value(expr);
            }
        }
    }

    fn record_std_callable_value(&mut self, expr: HirExprId) {
        if let Some(HirOwner::Expr(owner)) = self.hir.index().expr_owner.get(&expr)
            && matches!(self.hir.exprs.get(*owner), Some(HirExpr::Call { callee, .. }) if *callee == expr)
        {
            return;
        }
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) else {
            return;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return;
        };
        let Some(SymbolDef::ImportAlias { path, .. }) =
            self.hir.symbols.get(symbol).map(|symbol| &symbol.def)
        else {
            return;
        };
        let Some(declaration) = self.std_registry.lookup_qualified(path) else {
            return;
        };
        let Some(intrinsic) = declaration.intrinsic.as_ref() else {
            return;
        };
        let Some(SymbolTypeFact::Flow { signature }) =
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol)
        else {
            return;
        };
        // A polymorphic reference needs its own checked instantiation before
        // it can be selected independently of a call site.
        if !signature.generic_params.is_empty()
            && !self.types.facts.generic_instantiations.contains_key(&expr)
        {
            return;
        }
        let pure_constructor = intrinsic_pure_constructor(intrinsic, signature);
        let intrinsic = intrinsic.id.0;
        let Some(flow_type) = self.types.facts.expr_types.get(&expr).copied() else {
            return;
        };
        if !matches!(self.types.store.get(flow_type), Some(Type::Function(flow)) if flow.input.len() == signature.params.len())
        {
            return;
        }
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        let summary = match &declaration.decl {
            StdDecl::Flow(_) => self.std_flow_summary(expr, expr, span),
            StdDecl::Type(ty)
                if (declaration.kind == StdSymbolKind::Constructor
                    || ty.kind == TypeDeclKind::Wrapper)
                    && pure_constructor =>
            {
                Some(crate::EffectSummary::local())
            }
            _ => None,
        };
        if let Some(summary) = summary {
            self.inputs.std_callable_values.insert(
                expr,
                crate::StdCallableEffectFact {
                    value: expr,
                    symbol,
                    intrinsic,
                    flow_type,
                    summary,
                },
            );
        }
    }
}

fn intrinsic_pure_constructor(
    intrinsic: &etas_std::IntrinsicDescriptor,
    signature: &etas_types::CallableSignature,
) -> bool {
    intrinsic.purity == IntrinsicPurity::Pure
        && intrinsic.memory_access == IntrinsicMemoryAccess::None
        && intrinsic.runtime_requirement == IntrinsicRuntimeRequirement::None
        && matches!(
            intrinsic.dispatch,
            etas_std::IntrinsicDispatch::PureKernel | etas_std::IntrinsicDispatch::Runtime
        )
        && matches!(
            intrinsic.lowering,
            etas_std::LoweringHint::PureBuiltin | etas_std::LoweringHint::RuntimeCall
        )
        && signature.effects.as_ref().is_some_and(closed_empty_row)
        && signature
            .requested_actions
            .as_ref()
            .is_some_and(closed_empty_row)
}

fn closed_empty_row(row: &etas_types::EffectRowRef) -> bool {
    row.effects.is_empty() && row.tail.is_none()
}
