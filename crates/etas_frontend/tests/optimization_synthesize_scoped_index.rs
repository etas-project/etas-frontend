use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "mcp_id",
        include_str!("fixtures/optimization/synthesize_scoped_index/mcp_id.es"),
    ),
    (
        "pending_interrupt",
        include_str!("fixtures/optimization/synthesize_scoped_index/pending_interrupt.es"),
    ),
    (
        "shadow_scope",
        include_str!("fixtures/optimization/synthesize_scoped_index/shadow_scope.es"),
    ),
    (
        "tool_name",
        include_str!("fixtures/optimization/synthesize_scoped_index/tool_name.es"),
    ),
    (
        "model_capability",
        include_str!("fixtures/optimization/synthesize_scoped_index/model_capability.es"),
    ),
    (
        "workspace_file",
        include_str!("fixtures/optimization/synthesize_scoped_index/workspace_file.es"),
    ),
    (
        "resource_version",
        include_str!("fixtures/optimization/synthesize_scoped_index/resource_version.es"),
    ),
    (
        "memory_entry",
        include_str!("fixtures/optimization/synthesize_scoped_index/memory_entry.es"),
    ),
    (
        "provider_route",
        include_str!("fixtures/optimization/synthesize_scoped_index/provider_route.es"),
    ),
    (
        "policy_rule",
        include_str!("fixtures/optimization/synthesize_scoped_index/policy_rule.es"),
    ),
];

#[test]
fn ten_real_scoped_literal_indexes_resolve_static_probes() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("SynthesizeScopedIndex")
                .is_none(),
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("SynthesizeScopedIndex")
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
    input.options.optimization_level = if optimize_hir {
        etas_frontend::OptimizationLevel::O3
    } else {
        etas_frontend::OptimizationLevel::O0
    };
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.expect("checked optimization fixture")
}
