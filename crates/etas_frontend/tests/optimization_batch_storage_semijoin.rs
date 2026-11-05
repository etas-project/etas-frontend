use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_permissions",
        include_str!("fixtures/optimization/batch_storage_semijoin/tool_permissions.es"),
    ),
    (
        "memory_records",
        include_str!("fixtures/optimization/batch_storage_semijoin/memory_records.es"),
    ),
    (
        "checkpoints",
        include_str!("fixtures/optimization/batch_storage_semijoin/checkpoints.es"),
    ),
    (
        "model_routes",
        include_str!("fixtures/optimization/batch_storage_semijoin/model_routes.es"),
    ),
    (
        "mcp_servers",
        include_str!("fixtures/optimization/batch_storage_semijoin/mcp_servers.es"),
    ),
    (
        "policy_rules",
        include_str!("fixtures/optimization/batch_storage_semijoin/policy_rules.es"),
    ),
    (
        "workspace_files",
        include_str!("fixtures/optimization/batch_storage_semijoin/workspace_files.es"),
    ),
    (
        "provider_keys",
        include_str!("fixtures/optimization/batch_storage_semijoin/provider_keys.es"),
    ),
    (
        "trace_spans",
        include_str!("fixtures/optimization/batch_storage_semijoin/trace_spans.es"),
    ),
    (
        "plugin_states",
        include_str!("fixtures/optimization/batch_storage_semijoin/plugin_states.es"),
    ),
];

#[test]
fn ten_storage_probe_pairs_are_batched_into_semijoins() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("BatchStorageSemijoin")
                .is_none(),
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("BatchStorageSemijoin")
                .unwrap()
                .rewritten,
            1,
            "{name} optimized: {:?}",
            optimized.optimization_report
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("SynthesizeScopedIndex")
                .unwrap()
                .rewritten,
            0,
            "{name} batched pass owns rewrite"
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
