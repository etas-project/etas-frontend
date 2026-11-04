use etas_frontend::{Frontend, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 10] = [
    (
        "repo_map",
        include_str!("fixtures/optimization/quality_aware_context_policy/repo_map.es"),
    ),
    (
        "chat_history",
        include_str!("fixtures/optimization/quality_aware_context_policy/chat_history.es"),
    ),
    (
        "retrieval_pack",
        include_str!("fixtures/optimization/quality_aware_context_policy/retrieval_pack.es"),
    ),
    (
        "session_summary",
        include_str!("fixtures/optimization/quality_aware_context_policy/session_summary.es"),
    ),
    (
        "tool_catalog",
        include_str!("fixtures/optimization/quality_aware_context_policy/tool_catalog.es"),
    ),
    (
        "diagnostic_trace",
        include_str!("fixtures/optimization/quality_aware_context_policy/diagnostic_trace.es"),
    ),
    (
        "code_window",
        include_str!("fixtures/optimization/quality_aware_context_policy/code_window.es"),
    ),
    (
        "memory_recall",
        include_str!("fixtures/optimization/quality_aware_context_policy/memory_recall.es"),
    ),
    (
        "plan_history",
        include_str!("fixtures/optimization/quality_aware_context_policy/plan_history.es"),
    ),
    (
        "policy_summary",
        include_str!("fixtures/optimization/quality_aware_context_policy/policy_summary.es"),
    ),
];

const BUDGETED_PROGRAMS: [(&str, &str); 10] = [
    (
        "repo_map",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/repo_map.es"),
    ),
    (
        "chat_history",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/chat_history.es"),
    ),
    (
        "retrieval_pack",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/retrieval_pack.es"),
    ),
    (
        "session_summary",
        include_str!(
            "fixtures/optimization/quality_aware_context_policy_budget/session_summary.es"
        ),
    ),
    (
        "tool_catalog",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/tool_catalog.es"),
    ),
    (
        "diagnostic_trace",
        include_str!(
            "fixtures/optimization/quality_aware_context_policy_budget/diagnostic_trace.es"
        ),
    ),
    (
        "code_window",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/code_window.es"),
    ),
    (
        "memory_recall",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/memory_recall.es"),
    ),
    (
        "plan_history",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/plan_history.es"),
    ),
    (
        "policy_summary",
        include_str!("fixtures/optimization/quality_aware_context_policy_budget/policy_summary.es"),
    ),
];

#[test]
fn ten_unconsumed_context_candidates_are_pruned() {
    for (name, source) in PROGRAMS {
        let baseline = compile(source, false);
        let optimized = compile(source, true);
        assert_eq!(
            baseline
                .optimization_report
                .pass("QualityAwareContextPolicy")
                .unwrap()
                .rewritten,
            0,
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("QualityAwareContextPolicy")
                .unwrap()
                .rewritten,
            1,
            "{name} optimized: {:?}",
            optimized.optimization_report
        );
    }
}

#[test]
fn ten_used_context_candidates_honor_the_explicit_token_budget() {
    for (name, source) in BUDGETED_PROGRAMS {
        let baseline = compile_with_budget(source, None);
        let optimized = compile_with_budget(source, Some(8));
        assert_eq!(
            baseline
                .optimization_report
                .pass("QualityAwareContextPolicy")
                .unwrap()
                .rewritten,
            0,
            "{name} baseline"
        );
        assert_eq!(
            optimized
                .optimization_report
                .pass("QualityAwareContextPolicy")
                .unwrap()
                .rewritten,
            1,
            "{name} budgeted"
        );
    }
}

fn compile(source: &str, optimize_hir: bool) -> etas_frontend::CheckedProject {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.optimize_hir = optimize_hir;
    check(input)
}

fn compile_with_budget(source: &str, budget: Option<usize>) -> etas_frontend::CheckedProject {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.quality_context_token_budget = budget;
    check(input)
}

fn check(input: ProjectInput) -> etas_frontend::CheckedProject {
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.expect("checked optimization fixture")
}
