use std::sync::Arc;

use etas_cache::{ArtifactStore, ProjectRevision, TypedArtifactStore};
use etas_core::SourceId;

use super::FrontendSession;
use crate::{
    BODY_UNIT_KIND, CheckRequest, FrontendArtifactKey, FrontendArtifactKind, ProjectChangeSet,
    ProjectInput, SnapshotDetailLevel, SourceChange, SourceInput, SourceVersion,
};

const SOURCE: &str = "module main; flow helper(x: i32) -> i32 { return x + 1; } flow main() -> i32 { return helper(2); }";

#[test]
fn output_snapshot_and_cache_share_the_same_semantic_allocations() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
    let response = session.check(project, CheckRequest::default()).unwrap();
    let output = &response.output;
    let checked = output.checked.as_ref().unwrap();
    let snapshot = response.snapshot.as_ref().unwrap();
    let last_good = session.snapshot(project).unwrap();
    assert!(std::ptr::eq(&**snapshot, &*last_good));
    assert!(Arc::ptr_eq(&snapshot.checked, checked));
    assert!(Arc::ptr_eq(
        checked.hir.shared_program(),
        output.hir.as_ref().unwrap().hir.shared_program()
    ));
    assert!(Arc::ptr_eq(
        checked.hir.shared_index(),
        output.hir.as_ref().unwrap().hir.shared_index()
    ));
    assert!(std::ptr::eq(&checked.symbols, &checked.hir.symbols));
    assert!(std::ptr::eq(&checked.source_map, &checked.hir.source_map));
    assert!(std::ptr::eq(&checked.scopes, &checked.hir.scopes));
    assert!(Arc::ptr_eq(
        &checked.types,
        &output.types.as_ref().unwrap().facts
    ));
    assert!(Arc::ptr_eq(
        &checked.type_store,
        &output.types.as_ref().unwrap().store
    ));
    assert!(Arc::ptr_eq(
        &checked.effects,
        &output.effects.as_ref().unwrap().facts
    ));
    assert!(Arc::ptr_eq(
        &checked.effect_registry,
        &output.effect_pipeline_artifacts.as_ref().unwrap().registry
    ));
    assert!(Arc::ptr_eq(
        &snapshot.sources,
        output.sources.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &snapshot.modules,
        output.modules.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(&snapshot.modules, &checked.module_index));
    assert!(Arc::ptr_eq(&snapshot.units, output.units.as_ref().unwrap()));
    assert!(Arc::ptr_eq(
        &snapshot.resolved_paths,
        output.resolved_paths.as_ref().unwrap()
    ));

    let store = &session.projects[&project].store;
    let cached = store
        .get::<crate::CheckedProject>(
            &FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key(),
        )
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&cached.value, checked));
    let types = store
        .get::<etas_types::TypeOutput>(
            &FrontendArtifactKey::project(FrontendArtifactKind::TypeFacts).to_cache_key(),
        )
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&types.value.facts, &checked.types));
    assert!(Arc::ptr_eq(&types.value.store, &checked.type_store));
    let effects = store
        .get::<crate::SharedEffectOutput>(
            &FrontendArtifactKey::project(FrontendArtifactKind::EffectFacts).to_cache_key(),
        )
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&effects.value.facts, &checked.effects));
    for (unit, body) in &output.type_body_outputs {
        let key = FrontendArtifactKey::unit(
            FrontendArtifactKind::TypeFacts,
            etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
        )
        .to_cache_key();
        let cached = store
            .get::<Arc<etas_types::BodyTypeOutput>>(&key)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&cached.value, body));
    }
    for (unit, body) in &output.effect_body_outputs {
        let key = FrontendArtifactKey::unit(
            FrontendArtifactKind::EffectFacts,
            etas_utils::UnitKey::new(BODY_UNIT_KIND, unit.0 as u64),
        )
        .to_cache_key();
        let cached = store
            .get::<Arc<etas_effects::BodyEffectOutput>>(&key)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&cached.value, body));
    }
    let parsed = store
        .get::<crate::ParsedSource>(
            &FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, SourceId(0))
                .to_cache_key(),
        )
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(
        &parsed.value.parse,
        &output.parsed_sources[0].parse
    ));
}

#[test]
fn warm_reuse_failed_edit_and_project_close_preserve_then_release_last_good() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
    let first = session.check(project, CheckRequest::default()).unwrap();
    let second = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(second.output.checked.is_some(), "{:?}", second.diagnostics);
    for (unit, body) in &first.output.type_body_outputs {
        assert!(Arc::ptr_eq(body, &second.output.type_body_outputs[unit]));
    }
    let good = second.snapshot.as_ref().unwrap();
    let weak = Arc::downgrade(&good.checked);
    let old_source = good.sources.files[0].text.clone();
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: SourceId(0),
                    version: SourceVersion(1),
                    text: SOURCE.replace("helper(2)", "helper(true)"),
                }],
                dependency_overlay_changes: vec![],
                environment_change: None,
                option_changes: vec![],
            },
        )
        .unwrap();
    let failed = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(failed.output.checked.is_none());
    assert!(
        failed
            .diagnostics
            .diagnostics
            .iter()
            .any(|d| d.severity == etas_core::Severity::Error)
    );
    let retained = session.snapshot(project).unwrap();
    assert!(Arc::ptr_eq(&retained.checked, &good.checked));
    assert_eq!(retained.sources.files[0].text, old_source);
    let symbol = retained
        .symbols
        .iter()
        .find(|s| s.name == "helper")
        .unwrap()
        .id;
    assert!(retained.definition_target_for_symbol(symbol).is_some());
    assert_eq!(retained.revision, ProjectRevision(0));
    drop((first, second, failed, retained));
    assert!(weak.upgrade().is_some());
    session.close_project(project).unwrap();
    assert!(weak.upgrade().is_none());
    assert!(session.snapshot(project).is_none());
    assert!(session.close_project(project).is_err());
}

#[test]
fn unchanged_check_refreshes_memory_views_without_retaining_the_old_graph() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
    let first = session.check(project, CheckRequest::default()).unwrap();
    let old_checked = Arc::downgrade(first.output.checked.as_ref().unwrap());
    let old_hir = Arc::downgrade(first.output.hir.as_ref().unwrap().hir.shared_program());
    let old_types = Arc::downgrade(&first.output.types.as_ref().unwrap().facts);
    let old_effects = Arc::downgrade(&first.output.effects.as_ref().unwrap().facts);
    let old_registry = Arc::downgrade(
        &first
            .output
            .effect_pipeline_artifacts
            .as_ref()
            .unwrap()
            .registry,
    );
    let second = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(second.output.checked.is_some());
    for (unit, body) in &first.output.type_body_outputs {
        assert!(Arc::ptr_eq(body, &second.output.type_body_outputs[unit]));
    }
    drop(first);
    assert!(old_checked.upgrade().is_none());
    assert!(old_hir.upgrade().is_none());
    assert!(old_types.upgrade().is_none());
    assert!(old_effects.upgrade().is_none());
    assert!(old_registry.upgrade().is_none());
    let cached = session.projects[&project]
        .store
        .get::<crate::CheckedProject>(
            &FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key(),
        )
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(
        &cached.value,
        second.output.checked.as_ref().unwrap()
    ));
}

#[test]
fn one_shot_retention_is_independent_of_response_snapshot_detail() {
    for detail in [SnapshotDetailLevel::None, SnapshotDetailLevel::Full] {
        let mut session = FrontendSession::new();
        let project =
            session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
        let response = session
            .check(
                project,
                CheckRequest::one_shot().with_snapshot_detail(detail),
            )
            .unwrap();
        assert!(response.output.checked.is_some());
        assert_eq!(
            response.snapshot.is_some(),
            detail == SnapshotDetailLevel::Full
        );
        assert!(session.snapshot(project).is_none());
        let key = FrontendArtifactKey::project(FrontendArtifactKind::CheckedProject).to_cache_key();
        assert!(!session.projects[&project].store.contains(&key).unwrap());
        let weak = Arc::downgrade(response.output.checked.as_ref().unwrap());
        drop(response);
        assert!(weak.upgrade().is_none());
    }
}

#[test]
fn memory_instrumentation_observes_real_passes_without_changing_retention() {
    // A deterministic sampling backend verifies hook wiring, not heap consumption.
    fn sample() -> super::AllocationSnapshot {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SAMPLES: AtomicU64 = AtomicU64::new(0);
        super::AllocationSnapshot {
            allocations: SAMPLES.fetch_add(1, Ordering::Relaxed),
            ..Default::default()
        }
    }
    let mut session = FrontendSession::new();
    session.set_memory_probe(Some(super::FrontendMemoryProbe(sample)));
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
    let result = session.check(project, CheckRequest::one_shot()).unwrap();
    assert!(result.output.checked.is_some());
    assert!(result.memory.retained_revision.is_none());
    assert!(result.memory.before.unwrap().allocations < result.memory.after.unwrap().allocations);
    assert_eq!(result.memory.hir_index_builds, 2);
    assert!(
        result
            .memory
            .passes
            .iter()
            .any(|p| p.pass == "TypeCheckBodyPass" && p.unit.is_some())
    );
    let effects = result
        .memory
        .passes
        .iter()
        .find(|p| p.pass == "RunEffectPipelinePass")
        .unwrap();
    assert_eq!(effects.hir_index_builds, 0);
    assert!(
        result
            .memory
            .passes
            .iter()
            .all(|p| p.after.allocations > p.before.allocations)
    );
    session.set_memory_probe(None);
    let plain = session.check(project, CheckRequest::default()).unwrap();
    assert!(plain.output.checked.is_some());
    assert!(plain.memory.passes.is_empty());
    assert!(plain.memory.before.is_none());
    assert!(plain.memory.after.is_none());
    assert_eq!(plain.memory.retained_revision, Some(ProjectRevision(0)));
}
