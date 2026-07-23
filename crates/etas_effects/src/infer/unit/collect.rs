use std::collections::BTreeSet;

use etas_hir::{
    ExprView, HirBodyKind, HirExpr, HirExprId, HirItem, HirItemId, HirProgram, HirTreeView,
    HirVisitor, ResolveResult, SymbolDef, walk_body,
};
use etas_hir_analysis::HirAnalysisContext;
use etas_hir_analysis::unit::{HirAnonymousFlowBody, HirSemanticUnit, HirSemanticUnitCollector};

use super::{EffectAnonymousFlowBody, EffectUnit};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct EffectUnitCollector {
    units: BTreeSet<EffectUnit>,
    #[serde(skip)]
    static_anonymous_flow_symbols: BTreeSet<etas_hir::SymbolId>,
}

impl EffectUnitCollector {
    pub(crate) fn collect_with_context(
        hir: &HirProgram,
        context: &HirAnalysisContext,
    ) -> Vec<EffectUnit> {
        let view = context.view(hir);
        let semantic_units = HirSemanticUnitCollector::collect_with_context(hir, context);
        let mut collector = Self {
            static_anonymous_flow_symbols: static_anonymous_flow_symbols(hir, &semantic_units),
            ..Self::default()
        };
        collector.collect_semantic_units(&semantic_units);
        collector.collect_effect_specific_units(hir, &view);
        collector.units.into_iter().collect()
    }

    fn collect_semantic_units(&mut self, semantic_units: &[HirSemanticUnit]) {
        for unit in semantic_units {
            match unit {
                HirSemanticUnit::Item(item) | HirSemanticUnit::TopLevelLet(item) => {
                    self.units.insert(EffectUnit::Item(*item));
                }
                HirSemanticUnit::AnonymousFlow { owner, expr, body } => {
                    self.units.insert(EffectUnit::AnonymousFlow {
                        owner: *owner,
                        value: *expr,
                        body: effect_anonymous_flow_body(*body),
                    });
                }
                HirSemanticUnit::HandlerArm(_) => {
                    // Effect units need the owning item; that is supplied by the item body view.
                }
            }
        }
    }

    fn collect_effect_specific_units(&mut self, hir: &HirProgram, view: &HirTreeView<'_>) {
        for module in view.modules() {
            for item in module.items() {
                let owner = item.id();
                for body in item.bodies() {
                    if let HirBodyKind::HandlerArm(arm) = body.id().kind {
                        self.units.insert(EffectUnit::HandlerArm { owner, arm });
                    }
                    let mut visitor = EffectSpecificUnitVisitor {
                        collector: self,
                        hir,
                        owner,
                    };
                    walk_body(view, body.id(), &mut visitor);
                }
            }
        }
    }

    fn is_first_class_call_site(&self, hir: &HirProgram, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = hir.exprs.get(callee) else {
            return true;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol) = hir.symbols.get(symbol) else {
            return true;
        };
        match (&symbol.kind, &symbol.def) {
            (_, SymbolDef::Local { .. } | SymbolDef::TopLevelLet { .. }) => false,
            (_, SymbolDef::Item { item }) => !matches!(
                hir.items.get(*item),
                Some(HirItem::Flow(_) | HirItem::Agent(_) | HirItem::Tool(_))
            ),
            (_, SymbolDef::ImportAlias { .. }) => false,
            _ => true,
        }
    }

    fn is_static_anonymous_flow_call(&self, hir: &HirProgram, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol_data) = hir.symbols.get(symbol) else {
            return false;
        };
        match symbol_data.def {
            SymbolDef::Item { item } => {
                hir.items
                    .get(item)
                    .is_some_and(|item| matches!(item, HirItem::TopLevelLet(_)))
                    && self.static_anonymous_flow_symbols.contains(&symbol)
            }
            SymbolDef::TopLevelLet { .. }
            | SymbolDef::Local { .. }
            | SymbolDef::PatternBinding { .. } => {
                self.static_anonymous_flow_symbols.contains(&symbol)
            }
            _ => false,
        }
    }
}

struct EffectSpecificUnitVisitor<'collector, 'hir> {
    collector: &'collector mut EffectUnitCollector,
    hir: &'hir HirProgram,
    owner: HirItemId,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for EffectSpecificUnitVisitor<'_, 'hir> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        match expr.data() {
            HirExpr::Call { callee, .. } => {
                if self.collector.is_first_class_call_site(self.hir, *callee)
                    && !self
                        .collector
                        .is_static_anonymous_flow_call(self.hir, *callee)
                {
                    self.collector.units.insert(EffectUnit::FirstClassFlowCall {
                        owner: self.owner,
                        call: expr.id(),
                    });
                }
            }
            HirExpr::StageCompose { .. } => {
                self.collector.units.insert(EffectUnit::AnonymousFlow {
                    owner: self.owner,
                    value: expr.id(),
                    body: EffectAnonymousFlowBody::Expr(expr.id()),
                });
            }
            _ => {}
        }
    }
}

fn effect_anonymous_flow_body(body: HirAnonymousFlowBody) -> EffectAnonymousFlowBody {
    match body {
        HirAnonymousFlowBody::Expr(expr) => EffectAnonymousFlowBody::Expr(expr),
        HirAnonymousFlowBody::Block(block) => EffectAnonymousFlowBody::Block(block),
    }
}

fn static_anonymous_flow_symbols(
    hir: &HirProgram,
    semantic_units: &[HirSemanticUnit],
) -> BTreeSet<etas_hir::SymbolId> {
    let lambda_values = semantic_units
        .iter()
        .filter_map(|unit| match unit {
            HirSemanticUnit::AnonymousFlow { expr, .. } => Some(*expr),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    hir.symbols
        .iter()
        .filter_map(|symbol| {
            let initializer = match symbol.def {
                SymbolDef::TopLevelLet { initializer, .. }
                | SymbolDef::Local {
                    initializer: Some(initializer),
                    ..
                }
                | SymbolDef::PatternBinding {
                    initializer: Some(initializer),
                    ..
                } => initializer,
                _ => return None,
            };
            lambda_values.contains(&initializer).then_some(symbol.id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use etas_core::{SourceFile, SourceId};
    use etas_hir::{HirBodyKind, lower_program};
    use etas_hir_analysis::HirAnalysisContext;
    use etas_hir_analysis::unit::HirSemanticUnit;

    use super::*;

    #[test]
    fn effect_units_share_hir_tree_view_body_coverage_with_semantic_units() {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(0),
            None,
            r#"
effect Approval {
  action request() -> unit;
}

effect Network;

flow Tokens(value: i32) -> i32 {
  return value;
}

@model(model = "local")
agent Writer(input: string) -> string {
  return input;
}

let reusable = handler {
  Approval.request() => {
    resume;
  }
};

flow main(input: string) -> string {
  let lambda = input => input;

  match input {
    _ => input,
  }

  handle {
    perform Approval.request();
  } with {
    Approval.request() => {
      resume;
    }
  };

  let result = input ~> Writer limit Tokens(1);
  return lambda(result);
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

        let context = HirAnalysisContext::new(&hir);
        let view = context.view(&hir);
        let body_kinds = view
            .modules()
            .flat_map(|module| module.items())
            .flat_map(|item| item.bodies())
            .map(|body| body.id().kind)
            .collect::<Vec<_>>();
        assert!(
            body_kinds
                .iter()
                .any(|kind| matches!(kind, HirBodyKind::Lambda(_))),
            "TreeView must index lambda bodies"
        );
        assert!(
            body_kinds
                .iter()
                .any(|kind| matches!(kind, HirBodyKind::HandlerArm(_))),
            "TreeView must index handler arm bodies"
        );
        assert!(
            body_kinds
                .iter()
                .any(|kind| matches!(kind, HirBodyKind::MatchArm { .. })),
            "TreeView must index match arm bodies"
        );
        assert!(
            body_kinds
                .iter()
                .any(|kind| matches!(kind, HirBodyKind::StageLimit { .. })),
            "TreeView must index stage limit bodies"
        );

        let semantic_units =
            etas_hir_analysis::unit::HirSemanticUnitCollector::collect_with_context(&hir, &context);
        let effect_units = EffectUnitCollector::collect_with_context(&hir, &context)
            .into_iter()
            .collect::<BTreeSet<_>>();

        for semantic_unit in semantic_units {
            match semantic_unit {
                HirSemanticUnit::Item(item) | HirSemanticUnit::TopLevelLet(item) => {
                    assert!(
                        effect_units.contains(&EffectUnit::Item(item)),
                        "effect units must include semantic item unit {semantic_unit:?}"
                    );
                }
                HirSemanticUnit::AnonymousFlow { owner, expr, body } => {
                    assert!(
                        effect_units.contains(&EffectUnit::AnonymousFlow {
                            owner,
                            value: expr,
                            body: effect_anonymous_flow_body(body),
                        }),
                        "effect units must include semantic anonymous flow {semantic_unit:?}"
                    );
                }
                HirSemanticUnit::HandlerArm(_) => {
                    // Effect handler-arm units carry owner identity, so compare below through
                    // TreeView body ownership instead of the ownerless semantic unit.
                }
            }
        }

        for module in view.modules() {
            for item in module.items() {
                for body in item.bodies() {
                    if let HirBodyKind::HandlerArm(arm) = body.id().kind {
                        assert!(
                            effect_units.contains(&EffectUnit::HandlerArm {
                                owner: item.id(),
                                arm,
                            }),
                            "effect units must attach handler arm {arm:?} to owner {:?}",
                            item.id()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn static_lambda_call_detection_uses_resolved_symbols_not_owner_block_scan() {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(0),
            None,
            r#"
let top = value => value;

flow main(input: string) -> string {
  let local = value => value;
  if true {
    let nested = value => value;
    nested(input);
  }
  if true {
    let top = value => value;
    top(input);
  }
  top(input);
  return local(input);
}
"#,
        ));
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

        let context = HirAnalysisContext::new(&hir);
        let effect_units = EffectUnitCollector::collect_with_context(&hir, &context);
        let first_class_calls = effect_units
            .iter()
            .filter(|unit| matches!(unit, EffectUnit::FirstClassFlowCall { .. }))
            .collect::<Vec<_>>();

        assert!(
            first_class_calls.is_empty(),
            "static lambda calls through top-level, nested, and shadowed symbols must not become first-class call units: {first_class_calls:?}"
        );
    }
}
