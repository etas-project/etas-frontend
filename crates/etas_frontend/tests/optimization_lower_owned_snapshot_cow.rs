use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "model_config",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/model_config.es"),
    ),
    (
        "tool_registry",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/tool_registry.es"),
    ),
    (
        "session_state",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/session_state.es"),
    ),
    (
        "policy_set",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/policy_set.es"),
    ),
    (
        "mcp_catalog",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/mcp_catalog.es"),
    ),
    (
        "workspace_view",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/workspace_view.es"),
    ),
    (
        "provider_options",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/provider_options.es"),
    ),
    (
        "memory_index",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/memory_index.es"),
    ),
    (
        "checkpoint_state",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/checkpoint_state.es"),
    ),
    (
        "plugin_manifest",
        include_str!("fixtures/optimization/lower_owned_snapshot_cow/plugin_manifest.es"),
    ),
];

#[test]
fn ten_proven_immutable_owned_snapshots_are_lowered() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("LowerOwnedSnapshotCOW")
                .is_none(),
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("LowerOwnedSnapshotCOW")
                .unwrap()
                .rewritten,
            1,
            "{name} optimized: {:?}",
            optimized.optimization_report
        );
    }
}

fn compile(source: &str, optimize_hir: bool) -> etas_frontend::CheckedProject {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.optimization_level = if optimize_hir {
        etas_frontend::OptimizationLevel::O3
    } else {
        etas_frontend::OptimizationLevel::O0
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.expect("checked optimization fixture")
}
