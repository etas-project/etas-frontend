use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "tool_schema_catalog",
        include_str!("fixtures/optimization/prepare_versioned_request/tool_schema_catalog.es"),
    ),
    (
        "model_tool_mapping",
        include_str!("fixtures/optimization/prepare_versioned_request/model_tool_mapping.es"),
    ),
    (
        "retry_payload_normalization",
        include_str!(
            "fixtures/optimization/prepare_versioned_request/retry_payload_normalization.es"
        ),
    ),
    (
        "prompt_static_segment",
        include_str!("fixtures/optimization/prepare_versioned_request/prompt_static_segment.es"),
    ),
    (
        "provider_header_plan",
        include_str!("fixtures/optimization/prepare_versioned_request/provider_header_plan.es"),
    ),
    (
        "policy_projection",
        include_str!("fixtures/optimization/prepare_versioned_request/policy_projection.es"),
    ),
    (
        "model_capability_map",
        include_str!("fixtures/optimization/prepare_versioned_request/model_capability_map.es"),
    ),
    (
        "serializer_schema",
        include_str!("fixtures/optimization/prepare_versioned_request/serializer_schema.es"),
    ),
    (
        "plugin_tool_snapshot",
        include_str!("fixtures/optimization/prepare_versioned_request/plugin_tool_snapshot.es"),
    ),
    (
        "immutable_request_options",
        include_str!(
            "fixtures/optimization/prepare_versioned_request/immutable_request_options.es"
        ),
    ),
];

#[test]
fn ten_real_request_preparation_programs_are_hoisted() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert_eq!(
            baseline
                .optimization_report
                .pass("PrepareVersionedRequest")
                .unwrap()
                .rewritten,
            0,
            "{name} baseline"
        );
        assert!(
            optimized
                .optimization_report
                .pass("PrepareVersionedRequest")
                .unwrap()
                .rewritten
                >= 1,
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
