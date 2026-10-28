use etas_hir::{HirExpr, HirExprId, HirOwner, ResolveResult, SymbolDef};
use etas_std::StdDecl;
use etas_types::Type;

use super::engine::EffectSemantics;

impl EffectSemantics<'_> {
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
        let (StdDecl::Flow(flow), Some(intrinsic)) =
            (&declaration.decl, declaration.intrinsic.as_ref())
        else {
            return;
        };
        // A polymorphic reference needs its own checked instantiation before
        // it can be selected independently of a call site.
        if !flow.type_params.is_empty()
            && !self.types.facts.generic_instantiations.contains_key(&expr)
        {
            return;
        }
        let intrinsic = intrinsic.id.0;
        let Some(flow_type) = self.types.facts.expr_types.get(&expr).copied() else {
            return;
        };
        if !matches!(self.types.store.get(flow_type), Some(Type::Function(_))) {
            return;
        }
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        if let Some(summary) = self.std_flow_summary(expr, expr, span) {
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
