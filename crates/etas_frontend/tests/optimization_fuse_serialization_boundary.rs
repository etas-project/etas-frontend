use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "prompt_json",
        include_str!("fixtures/optimization/fuse_serialization_boundary/prompt_json.es"),
    ),
    (
        "tool_schema",
        include_str!("fixtures/optimization/fuse_serialization_boundary/tool_schema.es"),
    ),
    (
        "model_payload",
        include_str!("fixtures/optimization/fuse_serialization_boundary/model_payload.es"),
    ),
    (
        "trace_event",
        include_str!("fixtures/optimization/fuse_serialization_boundary/trace_event.es"),
    ),
    (
        "storage_record",
        include_str!("fixtures/optimization/fuse_serialization_boundary/storage_record.es"),
    ),
    (
        "checkpoint",
        include_str!("fixtures/optimization/fuse_serialization_boundary/checkpoint.es"),
    ),
    (
        "context_delta",
        include_str!("fixtures/optimization/fuse_serialization_boundary/context_delta.es"),
    ),
    (
        "rpc_envelope",
        include_str!("fixtures/optimization/fuse_serialization_boundary/rpc_envelope.es"),
    ),
    (
        "policy_document",
        include_str!("fixtures/optimization/fuse_serialization_boundary/policy_document.es"),
    ),
    (
        "plugin_manifest",
        include_str!("fixtures/optimization/fuse_serialization_boundary/plugin_manifest.es"),
    ),
];

#[test]
fn ten_json_stringify_parse_round_trips_are_fused() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("FuseSerializationBoundary")
                .is_none(),
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("FuseSerializationBoundary")
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
