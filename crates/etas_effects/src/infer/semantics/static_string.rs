use etas_hir::{HirExpr, HirExprId, HirFlowBody, HirItem, HirStmt, ResolveResult, SymbolDef};
use etas_hir_analysis::static_string::{
    StaticStringCallSemantics, StaticStringEvaluationError, StaticStringEvaluator,
    StaticStringOracle, StaticStringTransform,
};
use etas_std::{
    IntrinsicStaticStringSemantics, StaticStringTransform as StdStaticStringTransform,
    intrinsic_static_string_semantics,
};

use super::engine::EffectSemantics;

struct EffectStaticStringOracle<'a, 'hir> {
    semantics: &'a EffectSemantics<'hir>,
}

impl StaticStringOracle for EffectStaticStringOracle<'_, '_> {
    fn call_semantics(&self, callee: HirExprId) -> Option<StaticStringCallSemantics> {
        if let Some(semantics) = self.std_intrinsic_semantics(callee) {
            return Some(semantics);
        }
        self.source_flow_semantics(callee)
    }
}

impl EffectStaticStringOracle<'_, '_> {
    fn std_intrinsic_semantics(&self, callee: HirExprId) -> Option<StaticStringCallSemantics> {
        let HirExpr::Path(path) = self.semantics.hir.exprs.get(callee)? else {
            return None;
        };
        let path = self
            .semantics
            .canonical_static_resource_path_segments(path)?;
        let descriptor = self
            .semantics
            .std_registry
            .lookup_qualified(&path)?
            .intrinsic
            .as_ref()?;
        match intrinsic_static_string_semantics(descriptor.id)? {
            IntrinsicStaticStringSemantics::StringTransform {
                argument,
                transform,
            } => Some(StaticStringCallSemantics::StringTransform {
                argument,
                transform: match transform {
                    StdStaticStringTransform::Trim => StaticStringTransform::Trim,
                    StdStaticStringTransform::Lowercase => StaticStringTransform::Lowercase,
                    StdStaticStringTransform::Uppercase => StaticStringTransform::Uppercase,
                },
            }),
        }
    }

    fn source_flow_semantics(&self, callee: HirExprId) -> Option<StaticStringCallSemantics> {
        let HirExpr::Path(path) = self.semantics.hir.exprs.get(callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let item = match &self.semantics.hir.symbols.get(symbol)?.def {
            SymbolDef::Item { item } => *item,
            SymbolDef::ImportAlias {
                path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => self.semantics.source_item_for_path(path)?,
            _ => return None,
        };
        let HirItem::Flow(flow) = self.semantics.hir.items.get(item)? else {
            return None;
        };
        let return_expr = match flow.body {
            HirFlowBody::Expr { expr, .. } => expr,
            HirFlowBody::Block(block) => {
                let block = self.semantics.hir.blocks.get(block)?;
                match (block.stmts.split_last(), block.final_expr) {
                    (Some((last, prefix)), None)
                        if prefix.iter().all(|stmt| {
                            matches!(
                                self.semantics.hir.stmts.get(*stmt),
                                Some(HirStmt::Let { .. })
                            )
                        }) =>
                    {
                        match self.semantics.hir.stmts.get(*last)? {
                            HirStmt::Return {
                                value: Some(value), ..
                            } => *value,
                            _ => return None,
                        }
                    }
                    (_, Some(final_expr))
                        if block.stmts.iter().all(|stmt| {
                            matches!(
                                self.semantics.hir.stmts.get(*stmt),
                                Some(HirStmt::Let { .. })
                            )
                        }) =>
                    {
                        final_expr
                    }
                    _ => return None,
                }
            }
        };
        Some(StaticStringCallSemantics::SourceFlow {
            item,
            params: flow.params.clone(),
            return_expr,
        })
    }
}

pub(super) fn evaluate_static_string(
    semantics: &EffectSemantics<'_>,
    expr: HirExprId,
    projection: &[String],
) -> Result<String, StaticStringEvaluationError> {
    let oracle = EffectStaticStringOracle { semantics };
    StaticStringEvaluator::new(semantics.hir, &oracle).evaluate_string(expr, projection)
}
