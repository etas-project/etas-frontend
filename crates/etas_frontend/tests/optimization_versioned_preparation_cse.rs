use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "context_delta_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/context_delta_cache.es"),
    ),
    (
        "repo_map_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/repo_map_cache.es"),
    ),
    (
        "model_snapshot_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/model_snapshot_cache.es"),
    ),
    (
        "tool_schema_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/tool_schema_cache.es"),
    ),
    (
        "provider_capabilities_cache",
        include_str!(
            "fixtures/optimization/versioned_preparation_cse/provider_capabilities_cache.es"
        ),
    ),
    (
        "response_schema_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/response_schema_cache.es"),
    ),
    (
        "prompt_prefix_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/prompt_prefix_cache.es"),
    ),
    (
        "route_plan_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/route_plan_cache.es"),
    ),
    (
        "policy_projection_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/policy_projection_cache.es"),
    ),
    (
        "serializer_config_cache",
        include_str!("fixtures/optimization/versioned_preparation_cse/serializer_config_cache.es"),
    ),
];

#[test]
fn ten_real_versioned_preparations_are_commoned() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert!(
            baseline
                .optimization_report
                .pass("VersionedPreparationCSE")
                .is_none(),
            "{name} baseline"
        );
        assert!(
            optimized
                .optimization_report
                .pass("VersionedPreparationCSE")
                .unwrap()
                .rewritten
                >= 1,
            "{name} optimized report: {:?}",
            optimized.optimization_report
        );
    }
}

#[test]
fn commoned_generic_method_calls_reconcile_checked_call_facts() {
    let checked = compile(
        r#"
flow main() -> bool {
    let first: Range<u64> = Range.closed(0, 18446744073709551615);
    let same: Range<u64> = Range.closed(0, 18446744073709551615);
    return first == same;
}
"#,
        true,
    );
    assert_eq!(
        checked
            .optimization_report
            .pass("VersionedPreparationCSE")
            .unwrap()
            .rewritten,
        1
    );
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
