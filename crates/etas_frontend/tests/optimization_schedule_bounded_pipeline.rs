use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_calls",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/tool_calls.es"),
    ),
    (
        "model_batches",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/model_batches.es"),
    ),
    (
        "file_jobs",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/file_jobs.es"),
    ),
    (
        "mcp_requests",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/mcp_requests.es"),
    ),
    (
        "trace_events",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/trace_events.es"),
    ),
    (
        "retrieval_docs",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/retrieval_docs.es"),
    ),
    (
        "memory_writes",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/memory_writes.es"),
    ),
    (
        "policy_checks",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/policy_checks.es"),
    ),
    (
        "plugin_tasks",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/plugin_tasks.es"),
    ),
    (
        "checkpoint_jobs",
        include_str!("fixtures/optimization/schedule_bounded_pipeline/checkpoint_jobs.es"),
    ),
];

#[test]
fn ten_bounded_stage_pairs_are_scheduled_in_one_traversal() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert_eq!(
            baseline
                .optimization_report
                .pass("ScheduleBoundedPipeline")
                .unwrap()
                .rewritten,
            0,
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("ScheduleBoundedPipeline")
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
    input.options.optimize_hir = optimize_hir;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.expect("checked optimization fixture")
}
