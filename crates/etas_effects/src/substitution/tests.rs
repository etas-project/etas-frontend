use super::*;
use crate::{
    ActionEvent, ActionEventSource, ActionTraceDomain, CallableExitSummary, CallableOutcome,
    DefaultHandlerContribution, DefaultHandlerEffects, ExitTraceFact, OutcomeAnalysisGap,
    ParameterCallTarget, RequestInstantiationSummary, ReturnObservation, TraceSpecClauseFact,
};
use etas_core::{SourceId, Span, TextRange, TextSize};
use etas_hir::SymbolId;
use etas_types::{NamedTypeRef, PrimitiveType, Type, TypeInterner};
use std::collections::{BTreeSet, HashMap};

fn types() -> (etas_types::TypeStore, TypeId, TypeId) {
    let mut interner = TypeInterner::new();
    let parameter = interner.intern(Type::Named(NamedTypeRef {
        name: "T".into(),
        source_parameter: Some(SymbolId(31)),
    }));
    let boolean = interner.intern(Type::Primitive(PrimitiveType::Bool));
    (interner.into_store(), parameter, boolean)
}

fn span() -> Span {
    Span::new(SourceId(9), TextRange::new(TextSize(3), TextSize(7)))
}

fn apply(
    source: &EffectSummary,
    store: &etas_types::TypeStore,
    boolean: TypeId,
    registry: &EffectRegistry,
) -> Result<EffectSummary, SummarySubstitutionError> {
    let names = HashMap::new();
    let symbols = [(SymbolId(31), boolean)].into_iter().collect();
    let rows = [("E".into(), EffectRow::empty())];
    let engine = TypeSubstitutionEngine::existing(store, &names).with_source_parameters(&symbols);
    EffectSummarySubstitution::new(engine, registry, &rows, 4096, None).apply(source)
}

#[test]
fn full_summary_substitution_preserves_requests_exits_defaults_requirements_and_provenance() {
    let (store, parameter, boolean) = types();
    let registry = EffectRegistry::with_standard_effects();
    let action = registry.action_by_name("Fs.read").unwrap();
    let applied_action = |ty| {
        Effect::AppliedAction(crate::ActionInstanceRef {
            action: action.clone(),
            args: vec![EffectArgRef::Type(ty)],
        })
    };
    let source_action = applied_action(parameter);
    let expected_action = applied_action(boolean);
    let row = EffectRow::closed(EffectSet::one(source_action.clone()));
    let callback = ActionTraceDomain::ParameterCall {
        target: ParameterCallTarget::Native(SymbolId(17)),
        parameter: "callback".into(),
        span: span(),
    };
    let trace = ActionTraceDomain::Seq(vec![
        ActionTraceDomain::Event(ActionEvent {
            action: source_action.clone(),
            span: span(),
            source: ActionEventSource::Perform,
        }),
        callback.clone().repeat(),
    ]);
    let mut source = EffectSummary {
        requested_actions: row.clone(),
        default_actions: row.clone(),
        handled_actions: row.clone(),
        action_trace: trace,
        handler_requests: RequestInstantiationSummary::action(
            source_action.clone(),
            vec![("R".into(), parameter)],
            etas_types::ActionRequestSignature {
                params: vec![parameter],
                output: parameter,
            },
        ),
        default_handler_effects: Some(DefaultHandlerEffects {
            direct: EffectRow {
                effects: EffectSet::new(),
                open: Some(crate::effect_var_id_from_name("E")),
            },
            handlers: vec![DefaultHandlerContribution {
                action: source_action,
                produced: EffectRow::closed(EffectSet::one(Effect::Error(parameter))),
            }],
        }),
        exits: Some(CallableExitSummary {
            handler_gaps: Default::default(),
            parameter_exits: vec![crate::ParameterExitFact {
                target: ParameterCallTarget::Native(SymbolId(17)),
                output: parameter,
                source: span().into(),
                handler_free: Some(true),
                enclosing_handled: Some(crate::EffectRow::empty()),
                arguments: Some(vec![crate::ParameterExitArgument {
                    name: None,
                    ty: parameter,
                    source: span().into(),
                    value: None,
                }]),
                prefix: callback.clone(),
            }],
            exits: vec![ExitTraceFact {
                outcome: CallableOutcome::Returned {
                    ty: parameter,
                    observation: ReturnObservation::CallableSources(BTreeSet::from([
                        crate::CallableReturnSource::SourceItem(etas_hir::HirItemId(12)),
                    ])),
                },
                trace: callback.clone(),
            }],
            gaps: BTreeSet::from([OutcomeAnalysisGap::ParameterCall {
                target: ParameterCallTarget::Native(SymbolId(17)),
                output: parameter,
                source: span().into(),
            }]),
        }),
        requirements: crate::RequirementSet::one(crate::RequirementFact::TraceSpec(
            TraceSpecClauseFact::Deny {
                pattern: row.clone(),
                label: "source scope".into(),
            },
        )),
        trace_spec_obligations: crate::RequirementSet::one(crate::RequirementFact::TraceSpec(
            TraceSpecClauseFact::RequireBefore {
                guard: row.clone(),
                guard_label: "guard".into(),
                target: row,
                target_label: "target".into(),
            },
        )),
        ..EffectSummary::local()
    };
    source.refresh_escaping_effects();
    let result = apply(&source, &store, boolean, &registry).unwrap();
    for row in [
        &result.requested_actions,
        &result.default_actions,
        &result.handled_actions,
    ] {
        assert_eq!(
            *row,
            EffectRow::closed(EffectSet::one(expected_action.clone()))
        );
    }
    assert_eq!(
        result.escaping_effects,
        EffectRow::closed(EffectSet::one(Effect::Error(boolean)))
    );
    let RequestInstantiationSummary::Available { actions, .. } = &result.handler_requests else {
        panic!("request evidence must stay available");
    };
    let request = actions.first().unwrap();
    assert_eq!(request.action, expected_action);
    assert_eq!(request.type_bindings, Some(vec![("R".into(), boolean)]));
    assert_eq!(request.signature.as_ref().unwrap().output, boolean);
    let exit = &result.exits.as_ref().unwrap().exits[0];
    assert!(matches!(exit.outcome, CallableOutcome::Returned { ty, .. } if ty == boolean));
    let CallableOutcome::Returned { observation, .. } = &exit.outcome else {
        unreachable!()
    };
    let CallableOutcome::Returned {
        observation: original,
        ..
    } = &source.exits.as_ref().unwrap().exits[0].outcome
    else {
        unreachable!()
    };
    assert_eq!(
        observation, original,
        "source identity must not be re-keyed as a different checked reference"
    );
    assert_eq!(exit.trace, callback);
    assert!(result.exits.as_ref().unwrap().gaps.iter().any(
        |gap| matches!(gap, OutcomeAnalysisGap::ParameterCall { output, .. } if *output == boolean)
    ));
    let symbolic_exit = &result.exits.as_ref().unwrap().parameter_exits[0];
    assert_eq!(symbolic_exit.output, boolean);
    assert_eq!(symbolic_exit.arguments.as_ref().unwrap()[0].ty, boolean);
    assert_eq!(symbolic_exit.prefix, callback);
    assert_eq!(
        symbolic_exit.target,
        ParameterCallTarget::Native(SymbolId(17))
    );
    assert!(result.action_trace.action_set().contains(&expected_action));
    for requirement in result
        .requirements
        .iter()
        .chain(result.trace_spec_obligations.iter())
    {
        match requirement {
            crate::RequirementFact::TraceSpec(TraceSpecClauseFact::Deny { pattern, .. }) => {
                assert!(pattern.effects.contains(&expected_action))
            }
            crate::RequirementFact::TraceSpec(TraceSpecClauseFact::RequireBefore {
                guard,
                target,
                ..
            }) => {
                assert!(guard.effects.contains(&expected_action));
                assert!(target.effects.contains(&expected_action));
            }
            _ => panic!("expected substituted trace requirements"),
        }
    }
    assert_eq!(source.exits.as_ref().unwrap().exits[0].trace, callback);
    assert_ne!(result.requested_actions, source.requested_actions);
}

#[test]
fn unbound_rows_bad_action_arity_and_unknown_actions_fail_closed() {
    let (store, _, boolean) = types();
    let registry = EffectRegistry::with_standard_effects();
    let mut source = EffectSummary {
        escaping_effects: EffectRow {
            effects: EffectSet::new(),
            open: Some(crate::effect_var_id_from_name("Unknown")),
        },
        ..EffectSummary::local()
    };
    assert!(matches!(
        apply(&source, &store, boolean, &registry),
        Err(SummarySubstitutionError::UnboundRow(_))
    ));
    source.escaping_effects = EffectRow::empty();
    source
        .requested_actions
        .effects
        .insert(Effect::Action(registry.action_by_name("Fs.read").unwrap()));
    assert!(matches!(
        apply(&source, &store, boolean, &registry),
        Err(SummarySubstitutionError::InvalidAction(_))
    ));
    source.requested_actions.effects = EffectSet::one(Effect::Action(crate::ActionRef {
        tag: crate::EffectTagId(999_999),
        action: crate::EffectActionId(999_999),
    }));
    assert!(matches!(
        apply(&source, &store, boolean, &registry),
        Err(SummarySubstitutionError::InvalidAction(_))
    ));
    source.requested_actions = EffectRow::empty();
    source.handler_requests = RequestInstantiationSummary::Unavailable;
    assert_eq!(
        apply(&source, &store, boolean, &registry)
            .unwrap()
            .handler_requests,
        RequestInstantiationSummary::Unavailable
    );
}

#[test]
fn trace_substitution_is_iterative_and_keeps_widened_callback_placeholders() {
    let (store, parameter, boolean) = types();
    let registry = EffectRegistry::with_standard_effects();
    let action = registry.action_by_name("Fs.read").unwrap();
    let placeholder = crate::infer::domain::ParameterCallRef {
        target: ParameterCallTarget::Interface(0),
        parameter: "f".into(),
        span: span(),
    };
    let mut trace = ActionTraceDomain::Widened {
        actions: EffectSet::one(Effect::AppliedAction(crate::ActionInstanceRef {
            action: action.clone(),
            args: vec![EffectArgRef::Type(parameter)],
        })),
        parameter_calls: vec![placeholder.clone()],
    };
    for _ in 0..128 {
        trace = ActionTraceDomain::Repeat(Box::new(trace));
    }
    let result = apply(
        &EffectSummary {
            action_trace: trace,
            ..EffectSummary::local()
        },
        &store,
        boolean,
        &registry,
    )
    .unwrap();
    let mut trace = &result.action_trace;
    for _ in 0..128 {
        let ActionTraceDomain::Repeat(inner) = trace else {
            panic!("trace order changed");
        };
        trace = inner;
    }
    let ActionTraceDomain::Widened {
        actions,
        parameter_calls,
    } = trace
    else {
        panic!("widened trace was erased");
    };
    assert_eq!(parameter_calls, &[placeholder]);
    assert!(
        actions.contains(&Effect::AppliedAction(crate::ActionInstanceRef {
            action,
            args: vec![EffectArgRef::Type(boolean)]
        }))
    );
}

#[test]
fn row_arguments_and_type_binders_cannot_be_chosen_by_first_or_name_only_match() {
    let (store, parameter, boolean) = types();
    let registry = EffectRegistry::with_standard_effects();
    let names = HashMap::new();
    let symbols = [(SymbolId(31), boolean)].into_iter().collect();
    for rows in [
        vec![
            ("E".into(), EffectRow::empty()),
            ("E".into(), EffectRow::empty()),
        ],
        vec![(
            "E".into(),
            EffectRow {
                effects: EffectSet::new(),
                open: Some(crate::effect_var_id_from_name("E")),
            },
        )],
    ] {
        let engine =
            TypeSubstitutionEngine::existing(&store, &names).with_source_parameters(&symbols);
        assert!(matches!(
            EffectSummarySubstitution::new(engine, &registry, &rows, 100, None)
                .apply(&EffectSummary::local()),
            Err(SummarySubstitutionError::AmbiguousRow(_) | SummarySubstitutionError::UnboundRow(_))
        ));
    }
    let unrelated = [(SymbolId(99), boolean)].into_iter().collect();
    let engine =
        TypeSubstitutionEngine::existing(&store, &names).with_source_parameters(&unrelated);
    let source = EffectSummary {
        escaping_effects: EffectRow::closed(EffectSet::one(Effect::Error(parameter))),
        ..EffectSummary::local()
    };
    assert!(matches!(
        EffectSummarySubstitution::new(engine, &registry, &[], 100, None).apply(&source),
        Err(SummarySubstitutionError::Type(_))
    ));
}
