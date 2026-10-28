use super::*;
use etas_utils::PassControl;
use std::sync::Arc;

fn prepared() -> (
    ProjectContext,
    PassManager<ProjectContext>,
    etas_hir::HirExprId,
) {
    let mut context = ProjectContext::new(crate::ProjectInput::single_source(
        crate::SourceInput::anonymous(
            "module app; agent Worker() -> i32 { return 1; } flow main() -> unit { let definition = ^Worker; return; }",
        ),
    ));
    let mut manager = PassManager::new();
    let run = manager.run_pipeline(&mut crate::pipeline::project_pipeline(), &mut context);
    assert!(
        matches!(run.control, PassControl::Continue),
        "{:?}",
        run.control
    );
    assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
    assert!(run.records.iter().any(|record| {
        record.pass == "ResolveReflectionTargetsPass"
            && record.produced.contains(CHECKED_REFLECTION_TARGETS)
    }));
    let expr = *context
        .types
        .as_ref()
        .unwrap()
        .facts
        .reflections
        .keys()
        .next()
        .unwrap();
    (context, manager, expr)
}

fn rerun(context: &mut ProjectContext, manager: &mut PassManager<ProjectContext>) -> PassControl {
    context.checked = None;
    context.reflection_targets = None;
    ResolveReflectionTargetsPass
        .run(context, &PassContext::new(None), manager)
        .control
}

#[test]
fn missing_type_or_resolution_artifacts_prevent_reflection_integration() {
    let mut context = ProjectContext::new(crate::ProjectInput::single_source(
        crate::SourceInput::anonymous(""),
    ));
    let mut pipeline =
        etas_utils::Pipeline::new("missing_reflection_inputs").pass(ResolveReflectionTargetsPass);
    let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
    let PassControl::Failed(failure) = run.control else {
        panic!("missing reflection artifacts must fail before pass execution");
    };
    assert!(
        ResolveReflectionTargetsPass
            .descriptor()
            .requires
            .contains(failure.missing_artifact.unwrap())
    );
    assert!(context.reflection_targets.is_none());
}

#[test]
fn resolved_targets_share_type_facts_and_retain_the_real_expression_owner() {
    let (context, _, expr) = prepared();
    let checked = context.checked.as_ref().unwrap();
    let fact = &context.types.as_ref().unwrap().facts.reflections[&expr];
    assert!(std::ptr::eq(fact, &checked.checked_reflections()[&expr]));
    let (owner, span) = checked.reflection_site(expr).unwrap();
    assert_eq!(owner, checked.entry.unwrap());
    assert_eq!(
        span,
        checked
            .hir
            .exprs
            .get(expr)
            .unwrap()
            .span(&checked.hir.blocks)
    );
    assert!(
        crate::passes::BuildCheckedProjectPass
            .descriptor()
            .requires
            .contains(CHECKED_REFLECTION_TARGETS)
    );
}

#[test]
fn a_checked_target_cannot_lose_its_resolved_import_provenance() {
    let (mut context, mut manager, expr) = prepared();
    Arc::make_mut(context.resolved_paths.as_mut().unwrap())
        .expr_paths
        .retain(|path| path.expr != expr);
    assert!(matches!(
        rerun(&mut context, &mut manager),
        PassControl::Failed(_)
    ));
    assert!(context.reflection_targets.is_none());
}

#[test]
fn a_typed_reflection_without_a_solver_target_is_not_silently_skipped() {
    let (mut context, mut manager, expr) = prepared();
    Arc::make_mut(&mut context.types.as_mut().unwrap().facts)
        .reflections
        .remove(&expr);
    assert!(matches!(
        rerun(&mut context, &mut manager),
        PassControl::Failed(_)
    ));
    assert!(context.reflection_targets.is_none());
}

#[test]
fn an_analyzed_reflection_cannot_lose_both_its_type_and_target_facts() {
    let (mut context, mut manager, expr) = prepared();
    let facts = Arc::make_mut(&mut context.types.as_mut().unwrap().facts);
    facts.reflections.remove(&expr);
    facts.expr_types.remove(&expr);
    assert!(matches!(
        rerun(&mut context, &mut manager),
        PassControl::Failed(_)
    ));
}

#[test]
fn entry_reachable_scope_does_not_require_facts_for_an_unanalyzed_body() {
    let mut context = ProjectContext::new(crate::ProjectInput::single_source(
        crate::SourceInput::anonymous(
            "module app; agent Worker() -> i32 { return 1; } flow unused() -> unit { let other = ^Worker; return; } flow main() -> unit { let definition = ^Worker; return; }",
        ),
    ));
    context.check_scope = crate::CheckScope::EntryReachable;
    let run =
        PassManager::new().run_pipeline(&mut crate::pipeline::project_pipeline(), &mut context);
    assert!(
        matches!(run.control, PassControl::Continue),
        "{:?}",
        run.control
    );
    assert!(context.diagnostics.is_empty(), "{:?}", context.diagnostics);
    let checked = context.checked.unwrap();
    assert_eq!(
        checked
            .hir
            .exprs
            .iter()
            .filter(|(_, expr)| matches!(expr, etas_hir::HirExpr::Reflect(_)))
            .count(),
        2
    );
    assert_eq!(checked.checked_reflections().len(), 1);
}

#[test]
fn a_reflection_target_cannot_claim_a_different_module_access_scope() {
    let (mut context, mut manager, expr) = prepared();
    Arc::make_mut(&mut context.types.as_mut().unwrap().facts)
        .reflections
        .get_mut(&expr)
        .unwrap()
        .access_module = None;
    assert!(matches!(
        rerun(&mut context, &mut manager),
        PassControl::Failed(_)
    ));
}

#[test]
fn checked_project_cannot_reuse_an_index_from_another_type_fact_revision() {
    let (context, _, _) = prepared();
    let targets = context.reflection_targets.unwrap();
    let types = context.types.unwrap();
    assert!(targets.validate_owner(&types.facts).is_ok());
    assert!(
        targets
            .validate_owner(&Arc::new((*types.facts).clone()))
            .is_err()
    );
}

#[test]
fn target_integration_preserves_cancellation_and_resource_interruptions() {
    struct Interrupt(etas_utils::PipelineInterruptionKind);
    impl etas_utils::PipelineExecutionControl for Interrupt {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: self.0,
                reason: "stop reflection integration".into(),
            })
        }
    }
    let (context, _, _) = prepared();
    for kind in [
        etas_utils::PipelineInterruptionKind::Cancelled,
        etas_utils::PipelineInterruptionKind::ResourceLimit,
    ] {
        let result = CheckedReflectionTargets::resolve(
            &context.hir.as_ref().unwrap().hir,
            context.resolved_paths.as_ref().unwrap(),
            context.types.as_ref().unwrap(),
            &context.reachability.as_ref().unwrap().analysis_items,
            false,
            Some(&Interrupt(kind)),
        );
        assert!(
            matches!(result, Err(ReflectionTargetError::Interrupted(error)) if error.kind == kind)
        );
    }
}
