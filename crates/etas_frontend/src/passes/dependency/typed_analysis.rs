use std::collections::BTreeSet;

use etas_hir::{ExprView, HirItemId, HirVisitor, walk_item};
use etas_utils::{
    ArtifactSet, Pass, PassContext, PassControl, PassDescriptor, PassKind, PassManager, PassResult,
    Pipeline, PreservedArtifacts, UnitKey,
};

use crate::{
    BODY_UNIT_KIND, CheckScope, ProjectContext,
    passes::artifacts::{
        ANALYSIS_REACHABILITY_FACTS, HIR_OUTPUT, RESOLVED_PATHS, SIGNATURE_FACTS, TYPE_ENVIRONMENT,
        TYPE_OUTPUT,
    },
    passes::{FinalizeTypeFactsPass, TypeCheckBodyPass},
};

pub struct CompleteTypedAnalysisReachabilityPass;

impl Pass<ProjectContext> for CompleteTypedAnalysisReachabilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("CompleteTypedAnalysisReachabilityPass", PassKind::Transform)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                RESOLVED_PATHS,
                ANALYSIS_REACHABILITY_FACTS,
                SIGNATURE_FACTS,
                TYPE_ENVIRONMENT,
                TYPE_OUTPUT,
            ]))
            .produces(ArtifactSet::from([
                ANALYSIS_REACHABILITY_FACTS,
                TYPE_OUTPUT,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if context.check_scope != CheckScope::EntryReachable {
            return PassResult::unchanged();
        }
        let mut changed = false;
        loop {
            if let Some(control) = manager.execution_control()
                && let Err(interruption) = control.checkpoint()
            {
                return PassResult::interrupted(interruption);
            }
            let roots = match checked_targets(context) {
                Ok(roots) => roots,
                Err(reason) => return PassResult::failed(reason),
            };
            let (items, bodies) =
                match super::runtime_requirements::source_analysis_closure(context, roots) {
                    Ok(closure) => closure,
                    Err(reason) => return PassResult::failed(reason),
                };
            let Some(reachability) = &mut context.reachability else {
                return PassResult::failed("typed analysis requires entry reachability facts");
            };
            if items.is_subset(&reachability.analysis_items)
                && bodies.is_subset(&reachability.analysis_bodies)
            {
                break;
            }
            // This is a finite monotone closure over checked HIR bodies. Already
            // materialized body deltas are reused by the ordinary body passes.
            let added = bodies
                .difference(&reachability.analysis_bodies)
                .copied()
                .collect::<Vec<_>>();
            reachability.analysis_items.extend(items);
            reachability.analysis_bodies.extend(bodies);
            changed = true;
            let mut body = Pipeline::new("frontend.type_body").pass(TypeCheckBodyPass);
            for unit in added {
                let unit = Some(UnitKey::new(BODY_UNIT_KIND, unit.0.into()));
                let control = manager.run_nested_pipeline(&mut body, context, unit);
                if control != PassControl::Continue {
                    return PassResult {
                        control,
                        ..PassResult::unchanged()
                    };
                }
            }
            let mut completion =
                Pipeline::new("frontend.typed_reachable_bodies").pass(FinalizeTypeFactsPass);
            let control = manager.run_nested_pipeline(&mut completion, context, None);
            if control != PassControl::Continue {
                return PassResult {
                    control,
                    ..PassResult::unchanged()
                };
            }
        }
        if changed {
            PassResult::changed(
                PreservedArtifacts::All,
                ArtifactSet::from([ANALYSIS_REACHABILITY_FACTS, TYPE_OUTPUT]),
            )
        } else {
            PassResult::unchanged()
        }
    }
}

fn checked_targets(context: &ProjectContext) -> Result<Vec<HirItemId>, &'static str> {
    let (Some(hir), Some(types), Some(reachability)) =
        (&context.hir, &context.types, &context.reachability)
    else {
        return Err("typed analysis requires HIR, type and reachability facts");
    };
    struct Targets<'a> {
        facts: &'a etas_types::TypeFacts,
        hir: &'a etas_hir::HirProgram,
        store: &'a etas_types::TypeStore,
        items: BTreeSet<HirItemId>,
        missing: bool,
    }
    impl<'view, 'hir> HirVisitor<'view, 'hir> for Targets<'_> {
        fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
            if let etas_hir::HirExpr::Path(path) = expr.data()
                && let etas_hir::ResolveResult::Resolved(mut symbol) = path.resolution
            {
                let mut visited = BTreeSet::new();
                while let Some(target) = self.hir.source_import_targets.get(&symbol) {
                    if !visited.insert(symbol) {
                        self.missing = true;
                        return;
                    }
                    symbol = *target;
                }
                match self.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
                    Some(etas_hir::SymbolDef::TopLevelLet {
                        item,
                        classification,
                        ..
                    }) if !matches!(
                        classification,
                        etas_hir::TopLevelLetClassification::ResourceHandle(_)
                    ) =>
                    {
                        self.items.insert(*item);
                    }
                    Some(etas_hir::SymbolDef::Item { item })
                        if matches!(
                            self.hir.items.get(*item),
                            Some(etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_))
                        ) && matches!(
                            self.facts
                                .expr_types
                                .get(&expr.id())
                                .and_then(|ty| self.store.get(*ty)),
                            Some(etas_types::Type::Function(_))
                        ) =>
                    {
                        // Retain latent behavior without granting invocation authority.
                        self.items.insert(*item);
                    }
                    Some(etas_hir::SymbolDef::Item { item })
                        if matches!(
                            self.hir.items.get(*item),
                            Some(etas_hir::HirItem::Agent(_))
                        ) =>
                    {
                        self.items.insert(*item);
                    }
                    _ => {}
                }
            }
            if let Some(symbol) = self.facts.agent_method_calls.get(&expr.id()) {
                match self.facts.agent_methods.get(symbol) {
                    Some(method) => {
                        self.items.insert(method.body);
                    }
                    None => self.missing = true,
                }
            }
        }
    }
    let view = hir.view();
    let mut targets = Targets {
        facts: &types.facts,
        hir: &hir.hir,
        store: &types.store,
        items: reachability.analysis_items.clone(),
        missing: false,
    };
    for item in &reachability.analysis_items {
        walk_item(&view, *item, &mut targets);
    }
    if targets.missing
        || targets
            .items
            .iter()
            .any(|item| hir.hir.items.get(*item).is_none())
    {
        return Err("typed invocation has no checked source method body");
    }
    Ok(targets.items.into_iter().collect())
}
