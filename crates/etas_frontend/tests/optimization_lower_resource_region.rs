use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "model_client",
        include_str!("fixtures/optimization/lower_resource_region/model_client.es"),
    ),
    (
        "sql_pool",
        include_str!("fixtures/optimization/lower_resource_region/sql_pool.es"),
    ),
    (
        "http_session",
        include_str!("fixtures/optimization/lower_resource_region/http_session.es"),
    ),
    (
        "vector_store",
        include_str!("fixtures/optimization/lower_resource_region/vector_store.es"),
    ),
    (
        "tool_host",
        include_str!("fixtures/optimization/lower_resource_region/tool_host.es"),
    ),
    (
        "cache_handle",
        include_str!("fixtures/optimization/lower_resource_region/cache_handle.es"),
    ),
    (
        "trace_sink",
        include_str!("fixtures/optimization/lower_resource_region/trace_sink.es"),
    ),
    (
        "mcp_channel",
        include_str!("fixtures/optimization/lower_resource_region/mcp_channel.es"),
    ),
    (
        "checkpoint_store",
        include_str!("fixtures/optimization/lower_resource_region/checkpoint_store.es"),
    ),
    (
        "browser_context",
        include_str!("fixtures/optimization/lower_resource_region/browser_context.es"),
    ),
];

#[test]
fn ten_static_resources_are_lowered_to_outer_regions() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("LowerResourceRegion")
                .is_none(),
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("LowerResourceRegion")
                .unwrap()
                .rewritten,
            1,
            "{name} optimized: {:?}",
            optimized.optimization_report
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("PrepareVersionedRequest")
                .unwrap()
                .rewritten,
            0,
            "{name} resource pass owns the rewrite"
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
