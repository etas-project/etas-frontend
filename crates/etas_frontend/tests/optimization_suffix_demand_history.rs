use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "latest_cwd",
        include_str!("fixtures/optimization/suffix_demand_history/latest_cwd.es"),
    ),
    (
        "latest_tool_result",
        include_str!("fixtures/optimization/suffix_demand_history/latest_tool_result.es"),
    ),
    (
        "last_user_message",
        include_str!("fixtures/optimization/suffix_demand_history/last_user_message.es"),
    ),
    (
        "newest_model_config",
        include_str!("fixtures/optimization/suffix_demand_history/newest_model_config.es"),
    ),
    (
        "recent_checkpoint",
        include_str!("fixtures/optimization/suffix_demand_history/recent_checkpoint.es"),
    ),
    (
        "last_retry_error",
        include_str!("fixtures/optimization/suffix_demand_history/last_retry_error.es"),
    ),
    (
        "latest_workspace_root",
        include_str!("fixtures/optimization/suffix_demand_history/latest_workspace_root.es"),
    ),
    (
        "last_policy_decision",
        include_str!("fixtures/optimization/suffix_demand_history/last_policy_decision.es"),
    ),
    (
        "newest_memory_cursor",
        include_str!("fixtures/optimization/suffix_demand_history/newest_memory_cursor.es"),
    ),
    (
        "latest_provider_snapshot",
        include_str!("fixtures/optimization/suffix_demand_history/latest_provider_snapshot.es"),
    ),
];

#[test]
fn ten_real_history_suffix_scans_are_eliminated() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert_eq!(
            baseline
                .optimization_report
                .pass("SuffixDemandHistory")
                .unwrap()
                .rewritten,
            0,
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("SuffixDemandHistory")
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
