//! Run serially: cargo run -p etas_frontend --example compiler_memory -- 128
//! An optional second argument `one-shot` selects non-retaining checks.
//! A source-root path instead of a helper count measures a real source tree;
//! arguments 3/4/5 select the RAM budget, entry module and entry flow.
//! This allocator probe measures requested heap bytes, not RSS or native allocations.

#[path = "support/allocation_probe.rs"]
mod allocation_probe;
use allocation_probe::{reset_peak, sample};

use etas_cache::ProjectRevision;
use etas_core::SourceId;
use etas_frontend::{
    CheckRequest, DiskCacheAccess, FrontendMemoryProbe, FrontendSession, FrontendSessionOptions,
    ModulePath, ProjectChangeSet, ProjectEntry, ProjectInput, ProjectSourceLoadOptions,
    ProjectSourceLoadScope, ProjectSourceLoader, SnapshotDetailLevel, SourceChange, SourceInput,
    SourceVersion,
};
use serde_json::json;

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
    let target = std::env::args().nth(1).unwrap_or("128".to_owned());
    let helpers = target.parse::<usize>().ok();
    let one_shot = std::env::args().nth(2).as_deref() == Some("one-shot");
    let (input, stages) = if let Some(helpers) = helpers {
        assert!(helpers > 0);
        let text = source(helpers);
        let body_edit = text.replacen("x + 0", "x + 9", 1);
        let signature_edit = body_edit
            .replacen("f_0000(x: i32)", "f_0000(x: i32, extra: i32)", 1)
            .replace("f_0000(1)", "f_0000(1, 2)");
        let failed_edit = signature_edit.replacen("return y;", "return true;", 1);
        (
            ProjectInput::single_source(SourceInput::anonymous(text)),
            vec![
                ("cold", None),
                ("warm", None),
                ("body-edit", Some(body_edit)),
                ("signature-edit", Some(signature_edit)),
                ("failed-edit", Some(failed_edit)),
            ],
        )
    } else {
        let root = std::path::PathBuf::from(&target).canonicalize().unwrap();
        let input = ProjectSourceLoader::default()
            .load(ProjectSourceLoadOptions {
                project_root: root.clone(),
                roots: vec![root],
                entry: ProjectEntry {
                    module: std::env::args().nth(4).map(|module| ModulePath {
                        segments: module.split('.').map(str::to_owned).collect(),
                    }),
                    flow: std::env::args().nth(5).unwrap_or("main".to_owned()),
                },
                scope: ProjectSourceLoadScope::FullSourceTree,
                source_kind: None,
                import_search_roots: vec![],
                external_module_roots: vec![],
            })
            .unwrap()
            .input;
        (input, vec![("cold", None), ("warm", None)])
    };
    let budget = std::env::args()
        .nth(3)
        .map(|text| text.parse::<u64>().unwrap())
        .unwrap_or(etas_cache::DEFAULT_MEMORY_CACHE_BYTES);
    let mut session = FrontendSession::with_options(
        FrontendSessionOptions::memory_only().with_memory_cache_byte_budget(budget),
    )
    .unwrap();
    session.set_memory_probe(Some(FrontendMemoryProbe(sample)));
    let project = session.open_project(input);
    let mut revision = 0;
    for (stage, edit) in stages {
        let before = sample();
        reset_peak();
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
        let mut response = session
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
        let retained_revision_estimated_bytes = response.memory.retained_revision_estimated_bytes;
        let cache = response.memory.cache_after.take();
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
            "body_effect_exprs": body_effect_exprs, "project_effect_exprs": project_effect_exprs,
            "retained_revision_estimated_bytes": retained_revision_estimated_bytes,
            "cache": cache.map(|cache| json!({
                "budget_bytes": cache.budget_bytes,
                "estimated_retained_bytes": cache.estimated_retained_bytes,
                "payload_estimated_bytes": cache.payload_estimated_bytes,
                "bookkeeping_estimated_bytes": cache.bookkeeping_estimated_bytes,
                "entries": cache.entries,
                "evicted_artifacts": cache.evicted_artifacts,
                "rejected_insertions": cache.rejected_insertions,
                "by_kind": cache.by_kind.into_iter().map(|kind| json!({
                    "namespace": kind.namespace, "kind": kind.kind, "entries": kind.entries,
                    "referenced_payload_estimated_bytes": kind.referenced_payload_estimated_bytes,
                    "entry_bookkeeping_estimated_bytes": kind.entry_bookkeeping_estimated_bytes,
                })).collect::<Vec<_>>()
            }))})
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
