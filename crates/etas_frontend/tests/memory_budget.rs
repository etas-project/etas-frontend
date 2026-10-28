use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use etas_cache::ProjectRevision;
use etas_core::SourceId;
use etas_frontend::{
    CheckRequest, DiskCacheAccess, FrontendSession, FrontendSessionOptions, ProjectChangeSet,
    ProjectInput, SnapshotDetailLevel, SourceChange, SourceInput, SourceVersion,
};

const SOURCE: &str = "module main; flow helper(x: i32) -> i32 { return x + 1; } flow main() -> i32 { return helper(2); }";

#[test]
fn zero_and_small_cache_budgets_preserve_last_good_and_recompute_evicted_facts() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for mode in 0..3 {
        let disk = mode != 0;
        for budget in [0, 8192, 1024 * 1024] {
            let root = std::env::temp_dir().join(format!(
                "etas-memory-budget-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let options = if disk {
                FrontendSessionOptions::disk_cache(&root)
            } else {
                FrontendSessionOptions::memory_only()
            }
            .with_memory_cache_byte_budget(budget)
            .with_disk_cache_access(if mode == 2 {
                DiskCacheAccess::disabled()
            } else {
                DiskCacheAccess::read_write()
            });
            let mut session = FrontendSession::with_options(options).unwrap();
            let project =
                session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
            let request = CheckRequest::default().with_snapshot_detail(SnapshotDetailLevel::None);
            let first = session.check(project, request).unwrap();
            assert!(first.output.checked.is_some(), "{:?}", first.diagnostics);
            assert!(first.snapshot.is_none());
            for key in &first.cache.skipped_memory_budget_artifacts {
                assert!(!first.cache.stored_artifacts.contains(key));
            }
            if budget == 0 && mode != 1 {
                assert!(first.cache.stored_artifacts.is_empty());
                assert!(!first.cache.skipped_memory_budget_artifacts.is_empty());
            }
            let usage = session.memory_usage(project).unwrap();
            assert_eq!(usage.retained_revision, Some(ProjectRevision(0)));
            assert!(usage.retained_revision_estimated_bytes > budget);
            let cache = usage.cache.unwrap();
            assert_eq!(cache.budget_bytes, budget);
            assert!(cache.estimated_retained_bytes <= budget);
            assert!(cache.evicted_artifacts + cache.rejected_insertions > 0);
            if budget == 0 {
                assert_eq!(cache.entries, 0);
            }
            drop(first);

            let warm = session.check(project, CheckRequest::incremental()).unwrap();
            assert!(warm.output.checked.is_some(), "{:?}", warm.diagnostics);
            assert!(
                warm.memory
                    .cache_after
                    .as_ref()
                    .unwrap()
                    .estimated_retained_bytes
                    <= budget
            );
            drop(warm);

            for (revision, text) in [
                (1, SOURCE.replace("x + 1", "x + 2")),
                (2, SOURCE.replace("return x + 1;", "return true;")),
            ] {
                session
                    .apply_changes(
                        project,
                        ProjectChangeSet {
                            revision: ProjectRevision(revision),
                            source_changes: vec![SourceChange::Replace {
                                source: SourceId(0),
                                version: SourceVersion(revision),
                                text,
                            }],
                            dependency_overlay_changes: vec![],
                            environment_change: None,
                            option_changes: vec![],
                        },
                    )
                    .unwrap();
                let response = session.check(project, CheckRequest::incremental()).unwrap();
                assert_eq!(
                    response.output.checked.is_some(),
                    revision == 1,
                    "{:?}",
                    response.diagnostics
                );
                if revision == 2 {
                    assert!(
                        response
                            .diagnostics
                            .diagnostics
                            .iter()
                            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
                    );
                    assert_eq!(response.memory.retained_revision, Some(ProjectRevision(1)));
                }
                assert!(
                    response
                        .memory
                        .cache_after
                        .as_ref()
                        .unwrap()
                        .estimated_retained_bytes
                        <= budget
                );
            }

            let retained = session.snapshot(project).unwrap();
            assert_eq!(retained.revision, ProjectRevision(1));
            let weak = Arc::downgrade(&retained.checked);
            assert!(retained.sources.files[0].text.contains("x + 2"));
            let helper = retained
                .symbols
                .iter()
                .find(|symbol| symbol.name == "helper")
                .unwrap();
            assert!(retained.definition_target_for_symbol(helper.id).is_some());
            drop(retained);
            assert!(weak.upgrade().is_some());
            session.close_project(project).unwrap();
            assert!(weak.upgrade().is_none());
            assert!(session.memory_usage(project).is_none());
            drop(session);
            if disk {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}

#[test]
fn usage_attributes_real_shared_semantic_graphs_without_summing_shared_kinds() {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(SOURCE)));
    let response = session.check(project, CheckRequest::default()).unwrap();
    let usage = response.memory.cache_after.as_ref().unwrap();
    assert!(usage.entries > 0);
    assert!(usage.bookkeeping_estimated_bytes > 0);
    assert_eq!(
        usage.estimated_retained_bytes,
        usage.payload_estimated_bytes + usage.bookkeeping_estimated_bytes
    );
    let referenced: u64 = usage
        .by_kind
        .iter()
        .map(|kind| kind.referenced_payload_estimated_bytes)
        .sum();
    assert!(
        referenced > usage.payload_estimated_bytes,
        "shared HIR/type/effect roots must not be charged once per artifact kind"
    );
    assert!(
        usage
            .by_kind
            .iter()
            .any(|kind| kind.kind == "checked_project"
                && kind.referenced_payload_estimated_bytes > 0)
    );
    assert!(response.memory.retained_revision_estimated_bytes > 0);
    assert_eq!(response.memory.cache_before.as_ref().unwrap().entries, 0);
}
