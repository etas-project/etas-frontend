use std::cell::Cell;

use etas_code::{
    CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey, StableSourceKey,
    StableSourceOrigin,
    contract::CanonicalEffect,
    descriptor::EffectTargetInfo,
    residual::{monitor::CompiledTraceAlternatives, pattern::ActionPattern},
};
use etas_core::{DiagnosticCode, EffectDiagnosticCode};

use super::*;
use crate::trace_spec::{
    TracePatternProjection, TraceResidualCause, TraceSpecClause, lower_analyzed_trace_rules,
};
use crate::{EffectPipelineInput, EffectPipelineOutput, RunEffectPipeline};

const GATE: &str =
    "effect Gate { action check() -> unit; action send() -> unit; action extra() -> unit; }";

fn pattern(name: &str) -> ActionPattern {
    let owner = DefinitionKey::new(
        PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
        vec!["app".into()],
        vec!["Gate".into()],
        DefinitionKind::Effect,
    )
    .unwrap();
    ActionPattern::new(
        CanonicalEffect {
            target: EffectTargetInfo::Action {
                effect: owner,
                name: name.into(),
            },
            arguments: vec![],
        },
        ResidualPlanLimits::default().monitor.evidence.pattern,
    )
    .unwrap()
}

/// Test-only projection by action label. Production projection is the
/// checked canonical projector of the project layer.
#[derive(Default)]
struct LabelProjector {
    calls: Cell<usize>,
    /// Swap guard/target on this call number to model a forged projection.
    tamper_on_call: Option<usize>,
    unavailable: bool,
}

impl etas_core::memory::RetainedSize for LabelProjector {
    fn visit_retained<'a>(&'a self, _: &mut etas_core::memory::MemoryVisitor<'a>) {}
}

impl crate::TraceMonitorProjector for LabelProjector {
    fn project_trace_monitor(
        &self,
        request: crate::TraceMonitorProjectionRequest<'_>,
    ) -> Result<CompiledTraceAlternatives, crate::ResidualIdentityError> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if self.unavailable {
            return Err(crate::ResidualIdentityError::Unavailable(
                "unsupported pattern shape".into(),
            ));
        }
        let lowering = request.lowering;
        let source = lowering
            .dispositions
            .analyzed_lowering_source(
                lowering.type_fact_index,
                lowering.item,
                lowering.analysis,
                lowering.control,
            )
            .map_err(|error| crate::ResidualIdentityError::Unavailable(error.to_string()))?;
        let name = |label: &str| {
            let name = label.rsplit('.').next().unwrap_or(label).to_owned();
            if self.tamper_on_call == Some(call) {
                match name.as_str() {
                    "check" => "send".to_owned(),
                    "send" => "check".to_owned(),
                    _ => name,
                }
            } else {
                name
            }
        };
        let mut projections = Vec::new();
        for clause in source.iter().flatten() {
            let operands = match clause {
                TraceSpecClause::Allow { pattern, .. } | TraceSpecClause::Deny { pattern, .. } => {
                    vec![pattern]
                }
                TraceSpecClause::RequireBefore { guard, target, .. } => vec![guard, target],
                TraceSpecClause::RequireAfter {
                    target, obligation, ..
                } => vec![target, obligation],
                TraceSpecClause::Limit { .. } => vec![],
            };
            for operand in operands {
                projections.push(TracePatternProjection {
                    source: operand,
                    pattern: pattern(&name(&operand.label)),
                });
            }
        }
        lower_analyzed_trace_rules(lowering, projections)
            .map(|lowered| lowered.monitor)
            .map_err(|error| crate::ResidualIdentityError::Unavailable(format!("{error:?}")))
    }
}

struct Run {
    output: EffectPipelineOutput,
    item: etas_hir::HirItemId,
    index: usize,
    hir: etas_hir::HirRevision,
    text: String,
    key: StableSourceKey,
}

impl Run {
    fn snapshots(&self) -> [crate::EffectSourceSnapshot<'_>; 1] {
        [crate::EffectSourceSnapshot {
            source: etas_core::SourceId(7),
            key: self.key,
            text: &self.text,
        }]
    }
}

fn run(
    spec: &str,
    row: &str,
    body: &str,
    stable_source: bool,
    projector: Option<&dyn crate::TraceMonitorProjector>,
) -> Run {
    let text = format!(
        "module app.main;\n{GATE}\nspec Safe: trace = {spec};\nflow main() -> unit ![{row}] ~ Safe {{ {body} }}\n"
    );
    let source = etas_core::SourceId(7);
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(source, None, text.clone()));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let key = StableSourceKey::new(
        StableSourceOrigin::Project(CodeFingerprint::from_bytes([3; 32])),
        &["src".into(), "main.es".into()],
    )
    .unwrap();
    let snapshots = [crate::EffectSourceSnapshot {
        source,
        key,
        text: &text,
    }];
    let registry = etas_std::standard_registry();
    let output = RunEffectPipeline::run(EffectPipelineInput {
        source_snapshots: if stable_source { &snapshots } else { &[] },
        residual_identity: None,
        trace_monitor_projector: projector,
        frozen_calls: None,
        hir: &hir,
        types: &types,
        std_registry: &registry,
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .unwrap();
    let item = hir
        .items
        .iter()
        .find_map(|(id, item)| match item {
            etas_hir::HirItem::Flow(_) => Some(id),
            _ => None,
        })
        .unwrap();
    let index = types
        .facts
        .trace_spec_conformances
        .iter()
        .position(|fact| fact.item == item)
        .unwrap();
    Run {
        output,
        item,
        index,
        hir,
        text,
        key,
    }
}

fn eligibility(run: &Run) -> TraceResidualEligibility {
    classify_trace_residual(
        run.output.trace_specs.analysis(),
        run.output.trace_specs.dispositions(),
        run.index,
        run.item,
        None,
    )
    .unwrap()
}

fn has_incomplete(run: &Run) -> bool {
    run.output.effects.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
    })
}

fn rejected(run: &Run) -> bool {
    matches!(
        run.output.effects.facts.item_effects[&run.item].support,
        crate::InterpreterSupport::Rejected(_)
    )
}

const ORDERED: &str = "perform Gate.check(); perform Gate.send();";

#[test]
fn ordered_request_without_outcome_evidence_is_an_exact_runtime_obligation() {
    let run = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        true,
        None,
    );
    let TraceResidualEligibility::NeedsConcrete(requirement) = eligibility(&run) else {
        panic!("expected a residual requirement: {:?}", eligibility(&run));
    };
    assert_eq!(requirement.item(), run.item);
    assert_eq!(requirement.clauses().len(), 1);
    assert_eq!(
        requirement.clauses()[0].cause,
        TraceResidualCause::OutcomeEvidence
    );
    // Without a canonical projector there is no plan, so the conformance
    // keeps its static rejection.
    let plans = run.output.trace_specs.residual_plans();
    assert!(plans.plan(run.index).is_none());
    assert_eq!(
        plans.failure(run.index),
        Some(&ResidualPlanFailure::MissingProjector)
    );
    assert!(has_incomplete(&run));
    assert!(rejected(&run));
}

#[test]
fn definite_violations_missing_identity_and_violated_alternatives_are_not_residuals() {
    let wrong_order = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        "perform Gate.send(); perform Gate.check();",
        true,
        None,
    );
    assert_eq!(
        eligibility(&wrong_order),
        TraceResidualEligibility::Violated
    );

    let unrelated = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send, Gate.extra",
        "perform Gate.check(); perform Gate.send(); perform Gate.extra();",
        true,
        None,
    );
    assert_eq!(eligibility(&unrelated), TraceResidualEligibility::Violated);

    // The second alternative is definitely violated; a runtime OR branch must
    // not be able to accept it, so the whole conformance stays rejected.
    let mixed = run(
        "(+Gate.extra & (Gate.check >> Gate.send)) | (+Gate.check & +Gate.extra)",
        "Gate.check, Gate.send, Gate.extra",
        "perform Gate.check(); perform Gate.send(); perform Gate.extra();",
        true,
        None,
    );
    assert_eq!(
        eligibility(&mixed),
        TraceResidualEligibility::Unsupported(TraceResidualIneligibility::ViolatedAlternative)
    );

    let unstable = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        false,
        None,
    );
    assert_eq!(
        eligibility(&unstable),
        TraceResidualEligibility::Unsupported(TraceResidualIneligibility::MissingSourceIdentity)
    );
    for run in [&wrong_order, &unrelated, &mixed, &unstable] {
        assert!(
            run.output
                .trace_specs
                .residual_plans()
                .plan(run.index)
                .is_none()
        );
        assert!(
            run.output
                .trace_specs
                .residual_plans()
                .failure(run.index)
                .is_none()
        );
        assert!(rejected(run));
    }
}

#[test]
fn verified_plan_discharges_only_its_own_conformance_and_is_recorded_in_the_summary() {
    let projector = LabelProjector::default();
    let run = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        true,
        Some(&projector),
    );
    // Lowering and verification project independently.
    assert_eq!(projector.calls.get(), 2);
    let plans = run.output.trace_specs.residual_plans();
    let plan = plans.plan(run.index).expect("verified plan");
    assert_eq!(plan.item(), run.item);
    assert_eq!(plan.references().len(), 1);
    assert_eq!(plan.plan().obligations().len(), 1);
    assert_eq!(
        plan.contract(),
        etas_code::residual::identity::BehaviorContractRef::SourceConstraint
    );
    let reference = plan.references().iter().next().unwrap();
    assert!(
        run.output.effects.facts.item_effects[&run.item]
            .residual_checks
            .iter()
            .eq([reference])
    );
    assert!(
        !has_incomplete(&run),
        "{:#?}",
        run.output.effects.diagnostics
    );
    assert!(!rejected(&run));

    // The same program produces identical obligation and plan identities.
    let again = super::tests::run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        true,
        Some(&LabelProjector::default()),
    );
    let again_plan = again
        .output
        .trace_specs
        .residual_plans()
        .plan(again.index)
        .unwrap();
    assert_eq!(again_plan.plan().hash(), plan.plan().hash());
    assert_eq!(again_plan.references(), plan.references());

    // A definite violation is never discharged, even with a projector.
    let violated = super::tests::run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        "perform Gate.send(); perform Gate.check();",
        true,
        Some(&LabelProjector::default()),
    );
    assert!(violated.output.trace_specs.residual_plans().is_empty());
    assert!(rejected(&violated));
    assert!(
        violated.output.effects.facts.item_effects[&violated.item]
            .residual_checks
            .is_empty()
    );
}

#[test]
fn forged_or_unavailable_projection_leaves_the_static_rejection() {
    // The verification projection disagrees with the lowered monitor.
    let forged = LabelProjector {
        tamper_on_call: Some(2),
        ..Default::default()
    };
    let run_forged = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        true,
        Some(&forged),
    );
    let plans = run_forged.output.trace_specs.residual_plans();
    assert!(plans.plan(run_forged.index).is_none());
    assert!(matches!(
        plans.failure(run_forged.index),
        Some(ResidualPlanFailure::Verification(_))
    ));
    assert!(has_incomplete(&run_forged));
    assert!(rejected(&run_forged));
    assert!(
        run_forged.output.effects.facts.item_effects[&run_forged.item]
            .residual_checks
            .is_empty()
    );

    let unavailable = LabelProjector {
        unavailable: true,
        ..Default::default()
    };
    let run_unavailable = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        true,
        Some(&unavailable),
    );
    assert!(matches!(
        run_unavailable
            .output
            .trace_specs
            .residual_plans()
            .failure(run_unavailable.index),
        Some(ResidualPlanFailure::Projection(_))
    ));
    assert!(has_incomplete(&run_unavailable));
    assert!(rejected(&run_unavailable));
}

#[test]
fn verification_rejects_forged_lowered_plans_and_cancellation() {
    let projector = LabelProjector::default();
    let run = run(
        "Gate.check >> Gate.send",
        "Gate.check, Gate.send",
        ORDERED,
        true,
        Some(&projector),
    );
    let trace_specs = &run.output.trace_specs;
    let snapshots = run.snapshots();
    let registry = crate::EffectRegistry::with_standard_effects();
    let input = ResidualPlanInput {
        analysis: trace_specs.analysis(),
        dispositions: trace_specs.dispositions(),
        hir: &run.hir,
        registry: &registry,
        facts: &run.output.effects.facts,
        snapshots: &snapshots,
        projector: Some(&projector),
        limits: ResidualPlanLimits::default(),
        control: None,
    };
    let honest =
        verify_residual_plans(input, lower_residual_monitor_plans(input).unwrap()).unwrap();
    assert_eq!(
        honest.plan(run.index).unwrap().plan().hash(),
        trace_specs
            .residual_plans()
            .plan(run.index)
            .unwrap()
            .plan()
            .hash()
    );

    // A lowering result that omits a classified conformance is not a plan set.
    assert!(matches!(
        verify_residual_plans(input, ResidualMonitorPlans::default()),
        Err(EffectPipelineError::InvalidTraceSpecSourceSites { .. })
    ));

    let forged = |tamper: &dyn Fn(&mut LoweredTracePlan)| {
        let mut lowered = lower_residual_monitor_plans(input).unwrap();
        let Some(Ok(plan)) = lowered.lowered.get_mut(&run.index) else {
            panic!("lowered plan");
        };
        tamper(plan);
        verify_residual_plans(input, lowered).unwrap()
    };
    // Obligations recorded for a different clause.
    let wrong_clause = forged(&|plan| plan.references[0].1.clause_index += 1);
    // A contract reference not supplied by this compilation.
    let wrong_contract = forged(&|plan| {
        plan.contract = etas_code::residual::identity::BehaviorContractRef::FrozenInput(
            etas_code::residual::identity::BehaviorContractId::from_canonical_fingerprint(
                CodeFingerprint::from_bytes([9; 32]),
            ),
        )
    });
    // A monitor identity other than the checked projection.
    let wrong_monitor = forged(&|plan| {
        plan.monitor = etas_code::residual::identity::MonitorTemplateId::from_canonical_fingerprint(
            CodeFingerprint::from_bytes([8; 32]),
        )
    });
    for verified in [wrong_clause, wrong_contract, wrong_monitor] {
        assert!(verified.plan(run.index).is_none());
        assert!(matches!(
            verified.failure(run.index),
            Some(ResidualPlanFailure::Verification(_))
        ));
    }

    // An ineligible conformance cannot carry a lowered plan.
    let mut extra = lower_residual_monitor_plans(input).unwrap();
    extra.eligibility.insert(
        run.index,
        TraceResidualEligibility::Unsupported(TraceResidualIneligibility::UnresolvedCalls),
    );
    assert!(matches!(
        verify_residual_plans(input, extra),
        Err(EffectPipelineError::InvalidTraceSpecSourceSites { .. })
    ));

    struct Cancel;
    impl etas_utils::PipelineExecutionControl for Cancel {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "test cancellation".into(),
            })
        }
    }
    assert!(matches!(
        lower_residual_monitor_plans(ResidualPlanInput {
            control: Some(&Cancel),
            ..input
        }),
        Err(EffectPipelineError::Interrupted(_))
    ));
    let small = ResidualPlanLimits {
        max_total_obligations: 0,
        ..ResidualPlanLimits::default()
    };
    assert!(matches!(
        lower_residual_monitor_plans(ResidualPlanInput {
            limits: small,
            ..input
        }),
        Err(EffectPipelineError::AnalysisLimitExceeded { .. })
    ));
}
