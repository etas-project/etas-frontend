//! Run serially: cargo run -p etas_frontend --example compiler_memory -- 128
//! An optional second argument `one-shot` selects non-retaining checks.
//! This allocator probe measures requested heap bytes, not RSS or native allocations.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

use etas_cache::ProjectRevision;
use etas_core::SourceId;
use etas_frontend::{
    AllocationSnapshot, CheckRequest, DiskCacheAccess, FrontendMemoryProbe, FrontendSession,
    ProjectChangeSet, ProjectInput, SnapshotDetailLevel, SourceChange, SourceInput, SourceVersion,
};
use serde_json::json;

struct CountingAllocator;
static LIVE: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

fn allocated(size: usize) {
    let bytes = size as u64;
    TOTAL.fetch_add(bytes, Relaxed);
    COUNT.fetch_add(1, Relaxed);
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}

// Safety: every operation forwards its original allocation/layout to System;
// counters do not allocate or change ownership of the returned allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            allocated(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            allocated(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as u64, Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, old, size) };
        if !next.is_null() {
            LIVE.fetch_sub(old.size() as u64, Relaxed);
            allocated(size);
        }
        next
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn sample() -> AllocationSnapshot {
    AllocationSnapshot {
        live_bytes: LIVE.load(Relaxed),
        allocated_bytes: TOTAL.load(Relaxed),
        allocations: COUNT.load(Relaxed),
        peak_live_bytes: PEAK.load(Relaxed),
    }
}

fn source(helpers: usize) -> String {
    use std::fmt::Write;
    let mut text = "module main;\n".to_owned();
    for helper in 0..helpers {
        writeln!(
            text,
            "public flow f_{helper:04}(x: i32) -> i32 {{\n    let y = x + {};\n    return y;\n}}",
            helper % 10
        )
        .unwrap();
    }
    text.push_str("flow main() -> i32 { return f_0000(1); }\n");
    text
}

fn main() {
    let helpers: usize = std::env::args()
        .nth(1)
        .unwrap_or("128".to_owned())
        .parse()
        .unwrap();
    assert!(helpers > 0);
    let one_shot = std::env::args().nth(2).as_deref() == Some("one-shot");
    let text = source(helpers);
    let body_edit = text.replacen("x + 0", "x + 9", 1);
    let signature_edit = body_edit
        .replacen("f_0000(x: i32)", "f_0000(x: i32, extra: i32)", 1)
        .replace("f_0000(1)", "f_0000(1, 2)");
    let failed_edit = signature_edit.replacen("return y;", "return true;", 1);
    let mut session = FrontendSession::new();
    session.set_memory_probe(Some(FrontendMemoryProbe(sample)));
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(text)));
    let mut revision = 0;
    let stages = [
        ("cold", None),
        ("warm", None),
        ("body-edit", Some(body_edit)),
        ("signature-edit", Some(signature_edit)),
        ("failed-edit", Some(failed_edit)),
    ];
    for (stage, edit) in stages {
        let before = sample();
        PEAK.store(before.live_bytes, Relaxed);
        let started = std::time::Instant::now();
        if let Some(text) = edit {
            revision += 1;
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
        }
        let request = if one_shot {
            CheckRequest::one_shot()
        } else if stage == "cold" {
            CheckRequest::full_project()
        } else {
            CheckRequest::incremental()
        };
        let response = session
            .check(
                project,
                request
                    .with_disk_artifact_access(DiskCacheAccess::disabled())
                    .with_snapshot_detail(SnapshotDetailLevel::None),
            )
            .unwrap();
        let elapsed = started.elapsed();
        let after = sample();
        let checked = response.output.checked.is_some();
        assert_eq!(
            checked,
            stage != "failed-edit",
            "{:?}",
            response.diagnostics
        );
        if stage == "failed-edit" && !one_shot {
            assert_eq!(
                session.snapshot(project).unwrap().revision,
                ProjectRevision(revision - 1)
            );
        }
        let body_types: usize = response
            .output
            .type_body_outputs
            .values()
            .map(|b| b.types.len())
            .sum();
        let body_signatures: usize = response
            .output
            .type_body_outputs
            .values()
            .map(|b| b.facts.item_signatures.len())
            .sum();
        let body_effect_exprs: usize = response
            .output
            .effect_body_outputs
            .values()
            .map(|b| b.facts.expr_effects.len())
            .sum();
        let project_effect_exprs = response
            .output
            .effects
            .as_ref()
            .map(|e| e.facts.expr_effects.len());
        let reused = response.cache.reused_artifacts.len();
        let index_builds = response.memory.hir_index_builds;
        // Detailed pass samples are printed only on request, after measurement.
        if std::env::var_os("ETAS_MEMORY_PASSES").is_some() {
            for p in &response.memory.passes {
                println!(
                    "{}",
                    json!({"stage": stage, "pass": p.pass, "unit": p.unit.map(|u| u.id),
                    "live_before": p.before.live_bytes, "live_after": p.after.live_bytes,
                    "allocated": p.after.allocated_bytes - p.before.allocated_bytes,
                    "index_builds": p.hir_index_builds})
                );
            }
        }
        drop(response);
        let disposed = sample();
        println!(
            "{}",
            json!({"helpers": helpers, "stage": stage, "one_shot": one_shot,
            "checked": checked, "seconds": elapsed.as_secs_f64(),
            "live_before": before.live_bytes, "live_after": after.live_bytes,
            "peak_live": after.peak_live_bytes,
            "allocated_bytes": after.allocated_bytes - before.allocated_bytes,
            "allocations": after.allocations - before.allocations,
            "live_after_response_drop": disposed.live_bytes,
            "hir_index_builds": index_builds, "reused_artifacts": reused,
            "body_type_entries": body_types, "body_signatures": body_signatures,
            "body_effect_exprs": body_effect_exprs, "project_effect_exprs": project_effect_exprs})
        );
    }
    session.close_project(project).unwrap();
    let closed = sample();
    println!(
        "{}",
        json!({"stage": "project-close", "live_bytes": closed.live_bytes})
    );
    drop(session);
    let disposed = sample();
    println!(
        "{}",
        json!({"stage": "session-drop", "live_bytes": disposed.live_bytes})
    );
}
