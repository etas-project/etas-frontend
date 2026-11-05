use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "request_shape",
        include_str!("fixtures/optimization/agent_effect_legality/request_shape.es"),
    ),
    (
        "tool_schema",
        include_str!("fixtures/optimization/agent_effect_legality/tool_schema.es"),
    ),
    (
        "context_slice",
        include_str!("fixtures/optimization/agent_effect_legality/context_slice.es"),
    ),
    (
        "policy_projection",
        include_str!("fixtures/optimization/agent_effect_legality/policy_projection.es"),
    ),
    (
        "model_capability",
        include_str!("fixtures/optimization/agent_effect_legality/model_capability.es"),
    ),
    (
        "storage_key",
        include_str!("fixtures/optimization/agent_effect_legality/storage_key.es"),
    ),
    (
        "trace_label",
        include_str!("fixtures/optimization/agent_effect_legality/trace_label.es"),
    ),
    (
        "plugin_manifest",
        include_str!("fixtures/optimization/agent_effect_legality/plugin_manifest.es"),
    ),
    (
        "checkpoint_name",
        include_str!("fixtures/optimization/agent_effect_legality/checkpoint_name.es"),
    ),
    (
        "provider_header",
        include_str!("fixtures/optimization/agent_effect_legality/provider_header.es"),
    ),
];

#[test]
fn ten_real_pure_agent_expressions_receive_move_legality_facts() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("AgentEffectLegality")
                .is_none(),
            "{name} baseline"
        );
        assert!(
            optimized
                .optimization_report
                .pass("AgentEffectLegality")
                .unwrap()
                .matched
                > 0,
            "{name} optimized: {:?}",
            optimized.optimization_report
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("AgentEffectLegality")
                .unwrap()
                .rewritten,
            0,
            "{name} analysis gate does not rewrite HIR"
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
