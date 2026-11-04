use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_catalog",
        include_str!("fixtures/optimization/fuse_projection_scans/tool_catalog.es"),
    ),
    (
        "event_projection",
        include_str!("fixtures/optimization/fuse_projection_scans/event_projection.es"),
    ),
    (
        "memory_entries",
        include_str!("fixtures/optimization/fuse_projection_scans/memory_entries.es"),
    ),
    (
        "mcp_inventory",
        include_str!("fixtures/optimization/fuse_projection_scans/mcp_inventory.es"),
    ),
    (
        "model_capabilities",
        include_str!("fixtures/optimization/fuse_projection_scans/model_capabilities.es"),
    ),
    (
        "workspace_files",
        include_str!("fixtures/optimization/fuse_projection_scans/workspace_files.es"),
    ),
    (
        "policy_clauses",
        include_str!("fixtures/optimization/fuse_projection_scans/policy_clauses.es"),
    ),
    (
        "trace_events",
        include_str!("fixtures/optimization/fuse_projection_scans/trace_events.es"),
    ),
    (
        "repo_paths",
        include_str!("fixtures/optimization/fuse_projection_scans/repo_paths.es"),
    ),
    (
        "context_messages",
        include_str!("fixtures/optimization/fuse_projection_scans/context_messages.es"),
    ),
];

#[test]
fn ten_real_projection_scan_pairs_are_fused() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert_eq!(
            baseline
                .optimization_report
                .pass("FuseProjectionScans")
                .unwrap()
                .rewritten,
            0,
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("FuseProjectionScans")
                .unwrap()
                .rewritten,
            1,
            "{name} optimized report: {:?}",
            optimized.optimization_report
        );
    }
}

fn compile(source: &str, optimize_hir: bool) -> etas_frontend::CheckedProject {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.optimize_hir = optimize_hir;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.expect("checked optimization fixture")
}
