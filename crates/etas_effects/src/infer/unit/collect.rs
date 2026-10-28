use std::collections::BTreeSet;

use etas_hir::{ExprView, HirBodyKind, HirExpr, HirItemId, HirTreeView, HirVisitor, walk_body};
use etas_hir_analysis::HirAnalysisContext;
use etas_hir_analysis::unit::{HirAnonymousFlowBody, HirSemanticUnit, HirSemanticUnitCollector};

use super::{EffectAnonymousFlowBody, EffectUnit};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct EffectUnitCollector {
    units: BTreeSet<EffectUnit>,
}

impl EffectUnitCollector {
    pub(crate) fn collect_with_context(context: &HirAnalysisContext) -> Vec<EffectUnit> {
        let view = context.view();
        let semantic_units = HirSemanticUnitCollector::collect_with_context(context);
        let mut collector = Self::default();
        collector.collect_semantic_units(&semantic_units);
        collector.collect_effect_specific_units(&view);
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

    fn collect_effect_specific_units(&mut self, view: &HirTreeView<'_>) {
        for module in view.modules() {
            for item in module.items() {
                let owner = item.id();
                for body in item.bodies() {
                    if let HirBodyKind::HandlerArm(arm) = body.id().kind {
                        self.units.insert(EffectUnit::HandlerArm { owner, arm });
                    }
                    let mut visitor = EffectSpecificUnitVisitor {
                        collector: self,
                        owner,
                    };
                    walk_body(view, body.id(), &mut visitor);
                }
            }
        }
    }
}

struct EffectSpecificUnitVisitor<'collector> {
    collector: &'collector mut EffectUnitCollector,
    owner: HirItemId,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for EffectSpecificUnitVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if let HirExpr::StageCompose { .. } = expr.data() {
            self.collector.units.insert(EffectUnit::AnonymousFlow {
                owner: self.owner,
                value: expr.id(),
                body: EffectAnonymousFlowBody::Expr(expr.id()),
            });
        }
    }
}

fn effect_anonymous_flow_body(body: HirAnonymousFlowBody) -> EffectAnonymousFlowBody {
    match body {
        HirAnonymousFlowBody::Expr(expr) => EffectAnonymousFlowBody::Expr(expr),
        HirAnonymousFlowBody::Block(block) => EffectAnonymousFlowBody::Block(block),
    }
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
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

        let context = HirAnalysisContext::new(&hir);
        let view = context.view();
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
            etas_hir_analysis::unit::HirSemanticUnitCollector::collect_with_context(&context);
        let effect_units = EffectUnitCollector::collect_with_context(&context)
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
    fn calling_local_lambdas_does_not_create_additional_definition_units() {
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
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        assert!(hir.diagnostics.is_empty(), "{:#?}", hir.diagnostics);

        let context = HirAnalysisContext::new(&hir);
        let effect_units = EffectUnitCollector::collect_with_context(&context);
        assert_eq!(
            effect_units.len(),
            6,
            "only two items and four lambda definitions are analysis units: {effect_units:?}"
        );
        assert_eq!(
            effect_units
                .iter()
                .filter(|unit| matches!(unit, EffectUnit::AnonymousFlow { .. }))
                .count(),
            4
        );
    }
}
