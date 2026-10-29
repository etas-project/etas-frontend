use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::{
    EffectUnit,
    facts::{CallableEnvironment, CallableEnvironmentId},
    infer::semantics::engine::EffectSemantics,
};

fn with_semantics(
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
    test: impl FnOnce(&mut EffectSemantics<'_>, EffectUnit, EffectUnit, etas_hir::SymbolId, Span),
) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow factory(callback: () -> unit ![Console.stdout_write]) -> (() -> unit ![Console.stdout_write]) {
    return () => { perform Console.stdout_write("wrapper"); callback(); return; };
}
flow base() -> unit ![Console.stdout_write] { perform Console.stdout_write("base"); return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let std_registry = etas_std::standard_registry();
    let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
    let output = crate::RunEffectPipeline::run(crate::EffectPipelineInput {
        external_implementations: &[],
        frozen_calls: None,
        hir: &hir,
        types: &types,
        std_registry: &std_registry,
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .unwrap()
    .effects;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let mut semantics = EffectSemantics::new(&hir, &types, &std_registry, &registry, &[], &[]);
    semantics.inputs.unit_effects = output.facts.unit_effects;
    semantics.query_control = control;
    let (owner, parameter) = hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Flow(flow) if hir.symbols.get(flow.symbol)?.name == "factory" => {
                Some((item, flow.params[0]))
            }
            _ => None,
        })
        .unwrap();
    let value = hir
        .exprs
        .iter()
        .find_map(|(value, expression)| {
            matches!(expression, etas_hir::HirExpr::Lambda { .. }).then_some(value)
        })
        .unwrap();
    let unit = semantics
        .anonymous_flow_unit_for_expr(owner, value)
        .unwrap();
    let base = hir
        .items
        .iter()
        .find_map(|(item, declaration)| match declaration {
            etas_hir::HirItem::Flow(flow) if hir.symbols.get(flow.symbol)?.name == "base" => {
                Some(EffectUnit::Item(item))
            }
            _ => None,
        })
        .unwrap();
    let span = hir.exprs[value].span(&hir.blocks);
    test(&mut semantics, unit, base, parameter, span);
}

fn id(position: usize) -> CallableEnvironmentId {
    let key =
        etas_code::CodeFingerprint::of_bytes("test.callable-summary", &position.to_le_bytes());
    CallableEnvironmentId {
        origin: key,
        application: key,
    }
}

fn insert(
    semantics: &EffectSemantics<'_>,
    unit: EffectUnit,
    parameter: etas_hir::SymbolId,
    position: usize,
    origins: BTreeSet<LatentSource>,
) {
    semantics
        .record_callable_environment(
            id(position),
            CallableEnvironment {
                owner: unit.owner().unwrap(),
                parameters: [(ParameterCallTarget::Native(parameter), origins)].into(),
                generics: BTreeMap::new(),
            },
        )
        .unwrap();
}

fn captured(unit: EffectUnit, position: usize) -> LatentSource {
    LatentSource::Captured {
        unit,
        environment: id(position),
    }
}

#[test]
fn a_recursive_capture_component_converges_and_retains_its_grounded_actions() {
    with_semantics(None, |semantics, unit, base, parameter, span| {
        insert(
            semantics,
            unit,
            parameter,
            0,
            [captured(unit, 0), LatentSource::Unit(base)].into(),
        );
        let summary = semantics
            .solve_callable_summary(captured(unit, 0), span)
            .unwrap();
        let stdout = crate::Effect::Action(crate::ActionRef {
            tag: crate::CONSOLE_TAG,
            action: crate::CONSOLE_STDOUT_WRITE_ACTION,
        });
        assert!(summary.requested_actions.effects.contains(&stdout));
        assert!(summary.action_trace.action_set().contains(&stdout));
        let mut pending = vec![&summary.action_trace];
        let mut widened = false;
        while let Some(trace) = pending.pop() {
            match trace {
                ActionTraceDomain::UnknownOrder(_) | ActionTraceDomain::Widened { .. } => {
                    widened = true
                }
                ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                    pending.extend(parts)
                }
                ActionTraceDomain::Repeat(inner) => pending.push(inner),
                _ => {}
            }
        }
        assert!(
            widened,
            "the recursive component must retain its widened footprint"
        );
        assert_eq!(
            semantics
                .solve_callable_summary(captured(unit, 0), span)
                .unwrap(),
            summary
        );
    });
}

#[test]
fn ten_thousand_captured_dependencies_use_a_worklist_not_the_native_stack() {
    with_semantics(None, |semantics, unit, base, parameter, span| {
        for position in 0..10_000 {
            let next = if position == 9_999 {
                LatentSource::Unit(base)
            } else {
                captured(unit, position + 1)
            };
            insert(semantics, unit, parameter, position, [next].into());
        }
        let summary = semantics
            .solve_callable_summary(captured(unit, 0), span)
            .unwrap();
        assert!(!summary.requested_actions.is_empty());
        assert!(semantics.callable_summaries.borrow().summaries.len() >= 10_000);
    });
}

#[test]
fn an_unavailable_dependency_cannot_publish_a_partial_pure_summary() {
    with_semantics(None, |semantics, unit, _, parameter, span| {
        insert(
            semantics,
            unit,
            parameter,
            0,
            [LatentSource::Unavailable {
                expr: etas_hir::HirExprId(u32::MAX),
            }]
            .into(),
        );
        assert!(
            semantics
                .solve_callable_summary(captured(unit, 0), span)
                .is_err()
        );
        assert!(semantics.callable_summaries.borrow().summaries.is_empty());
        assert!(
            semantics
                .solve_callable_summary_in_mode(
                    captured(unit, 0),
                    span,
                    CallableSummaryMode::Descriptor
                )
                .is_err()
        );
        assert!(semantics.callable_summaries.borrow().summaries.is_empty());
    });
}

#[test]
fn a_symbolic_descriptor_does_not_satisfy_an_unbound_invocation() {
    with_semantics(None, |semantics, unit, _, parameter, span| {
        semantics
            .record_callable_environment(
                id(0),
                CallableEnvironment {
                    owner: unit.owner().unwrap(),
                    parameters: BTreeMap::new(),
                    generics: BTreeMap::new(),
                },
            )
            .unwrap();
        let descriptor = semantics
            .solve_callable_summary_in_mode(
                captured(unit, 0),
                span,
                CallableSummaryMode::Descriptor,
            )
            .unwrap();
        let mut pending = vec![&descriptor.action_trace];
        let mut found = false;
        while let Some(trace) = pending.pop() {
            match trace {
                ActionTraceDomain::ParameterCall { target, .. } => {
                    found |= *target == ParameterCallTarget::Native(parameter)
                }
                ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                    pending.extend(parts)
                }
                ActionTraceDomain::Repeat(inner) => pending.push(inner),
                _ => {}
            }
        }
        assert!(
            found,
            "a descriptor must retain the lexical parameter obligation"
        );
        assert!(
            semantics
                .solve_callable_summary(captured(unit, 0), span)
                .is_err(),
            "an invocation cannot reuse a symbolic descriptor as a closed solution"
        );
        assert!(
            semantics
                .callable_summaries
                .borrow()
                .summaries
                .keys()
                .all(|key| key.mode == CallableSummaryMode::Descriptor)
        );
    });
}

#[test]
fn cancellation_during_callable_solving_does_not_publish_provisional_summaries() {
    struct Stop(AtomicUsize);
    impl etas_utils::PipelineExecutionControl for Stop {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            if self.0.fetch_add(1, Ordering::SeqCst) >= 4 {
                return Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "stop captured summary analysis".into(),
                });
            }
            Ok(())
        }
    }
    let stop = Stop(AtomicUsize::new(0));
    with_semantics(Some(&stop), |semantics, unit, base, parameter, span| {
        insert(
            semantics,
            unit,
            parameter,
            0,
            [captured(unit, 0), LatentSource::Unit(base)].into(),
        );
        assert!(matches!(
            semantics.solve_callable_summary(captured(unit, 0), span),
            Err(EffectSpecializationError::Interrupted(_))
        ));
        assert!(semantics.callable_summaries.borrow().summaries.is_empty());
        assert!(semantics.callable_summaries.borrow().interruption.is_some());
    });
}
