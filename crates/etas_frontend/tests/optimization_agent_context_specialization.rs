use etas_frontend::{Frontend, OptimizationLevel, ProjectInput, SourceInput};

const PROGRAMS: [(&str, &str); 11] = [
    (
        "chat_triage",
        include_str!("fixtures/optimization/agent_context_specialization/chat_triage.es"),
    ),
    (
        "checkpoint_resume",
        include_str!("fixtures/optimization/agent_context_specialization/checkpoint_resume.es"),
    ),
    (
        "code_review",
        include_str!("fixtures/optimization/agent_context_specialization/code_review.es"),
    ),
    (
        "mcp_dispatch",
        include_str!("fixtures/optimization/agent_context_specialization/mcp_dispatch.es"),
    ),
    (
        "memory_recall",
        include_str!("fixtures/optimization/agent_context_specialization/memory_recall.es"),
    ),
    (
        "model_routing",
        include_str!("fixtures/optimization/agent_context_specialization/model_routing.es"),
    ),
    (
        "policy_gate",
        include_str!("fixtures/optimization/agent_context_specialization/policy_gate.es"),
    ),
    (
        "release_planning",
        include_str!("fixtures/optimization/agent_context_specialization/release_planning.es"),
    ),
    (
        "retrieval_route",
        include_str!("fixtures/optimization/agent_context_specialization/retrieval_route.es"),
    ),
    (
        "trace_diagnosis",
        include_str!("fixtures/optimization/agent_context_specialization/trace_diagnosis.es"),
    ),
    (
        "workspace_search",
        include_str!("fixtures/optimization/agent_context_specialization/workspace_search.es"),
    ),
];

const PASSES: [(&str, OptimizationLevel); 5] = [
    ("ProjectAgentObservation", OptimizationLevel::O1),
    ("PruneSupersededAgentEvents", OptimizationLevel::O1),
    ("ShortCircuitAgentThreshold", OptimizationLevel::O1),
    ("SpecializeAgentToolSchemas", OptimizationLevel::O2),
    ("SpecializeDeterministicDispatch", OptimizationLevel::O2),
];

#[test]
fn mixed_agent_workflows_exercise_each_specialization_at_least_ten_times() {
    assert!(PROGRAMS.len() >= 10);
    for (name, source) in PROGRAMS {
        let baseline = compile(source, OptimizationLevel::O0);
        let optimized = compile(source, OptimizationLevel::O2);
        for (pass, minimum_level) in PASSES {
            assert!(
                baseline.optimization_report.pass(pass).is_none(),
                "{name} {pass} baseline"
            );
            assert!(OptimizationLevel::O2.enables(minimum_level));
            assert_eq!(
                optimized
                    .optimization_report
                    .pass(pass)
                    .unwrap_or_else(|| panic!("{name} missing {pass}"))
                    .rewritten,
                1,
                "{name} {pass}: {:?}",
                optimized.optimization_report
            );
        }
    }
}

fn compile(source: &str, level: OptimizationLevel) -> etas_frontend::CheckedProject {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(source));
    input.options.optimization_level = level;
    let output = Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output
        .checked
        .expect("checked agent specialization fixture")
}
