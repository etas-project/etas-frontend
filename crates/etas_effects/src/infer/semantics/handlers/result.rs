use std::collections::BTreeSet;

use etas_hir::HirItemId;

use super::*;
use crate::ActionTraceDomain;
use crate::facts::CallableValueOrigin;
use crate::infer::semantics::specialize::EffectSpecializationError;

struct CheckedHandlerArm<'a> {
    fact: &'a HandlerArmFact,
    action: &'a crate::Effect,
    unconditional: bool,
}

impl EffectSemantics<'_> {
    fn checked_handler_arms(
        &self,
        handler: HirExprId,
        owner: Option<HirItemId>,
    ) -> Result<Option<Vec<CheckedHandlerArm<'_>>>, EffectSpecializationError> {
        let (Some(HirExpr::Handler { handlers, .. }), Some(fact), Some(owner)) = (
            self.hir.exprs.get(handler),
            self.inputs.handler_values.get(&handler),
            owner,
        ) else {
            return Ok(None);
        };
        if !handler_arm_units_match(owner, handlers, fact.arms.iter().map(|fact| fact.unit)) {
            return Err(EffectSpecializationError::InvalidCallableEnvironment {
                reason: "handler arm evidence does not match its checked analysis unit".into(),
            });
        }
        let mut result = Vec::with_capacity(handlers.len());
        for (arm, fact) in handlers.iter().zip(&fact.arms) {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectSpecializationError::Interrupted)?;
            }
            let arm_data = self.hir.handler_arms.get(*arm).ok_or_else(|| {
                EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "checked handler arm is missing from HIR".into(),
                }
            })?;
            let action = self.inputs.handler_action_effects.get(arm).ok_or_else(|| {
                EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "checked handler arm is missing its action effect".into(),
                }
            })?;
            let mut unconditional = true;
            for pattern in &arm_data.patterns {
                if let Some(control) = self.query_control {
                    control
                        .checkpoint()
                        .map_err(EffectSpecializationError::Interrupted)?;
                }
                let pattern = self.hir.pats.get(*pattern).ok_or_else(|| {
                    EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "checked handler arm is missing its payload pattern".into(),
                    }
                })?;
                unconditional &= matches!(
                    pattern,
                    etas_hir::HirPat::Binding { .. } | etas_hir::HirPat::Wildcard { .. }
                );
            }
            result.push(CheckedHandlerArm {
                fact,
                action,
                unconditional,
            });
        }
        Ok(Some(result))
    }

    pub(super) fn record_handle_result_sources(
        &self,
        expr: HirExprId,
        handler: HirExprId,
        body_normal: bool,
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        let Some(HirExpr::Handle { body, .. }) = self.hir.exprs.get(expr) else {
            return Ok(());
        };
        if !self.expr_type_contains_flow(expr) && !self.symbolic_contract_expr(state, *body) {
            return Ok(());
        }
        let mut sources = BTreeSet::new();
        let mut stopped = crate::EffectSet::new();
        let arms = self.checked_handler_arms(handler, state.owner)?;
        if let Some(arms) = &arms {
            for arm in arms {
                if arm.fact.completion.resumes == crate::ResumeSummary::None
                    && arm.fact.completion.finishes != crate::FinishSummary::None
                    && arm.unconditional
                {
                    stopped.insert(arm.action.clone());
                }
            }
        }
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let body_normal = body_normal
            && may_avoid(
                &state.summary.action_trace,
                &|action| stopped.iter().any(|stop| coverage.covers(stop, action)),
                self.query_control,
            )?;
        if body_normal {
            match self.latent_sources_for_expr(state, *body) {
                Some(found) => sources.extend(found),
                None => {
                    sources.insert(CallableValueOrigin::Unavailable { expr: *body });
                }
            }
        }
        match arms {
            Some(arms) => {
                for arm in arms {
                    if arm.fact.completion.finishes == crate::FinishSummary::None {
                        continue;
                    }
                    if let Some(crate::RequestInstantiationSummary::Available {
                        actions,
                        parameters,
                    }) = self.inputs.handled_body_requests.get(body)
                        && parameters.is_empty()
                        && actions
                            .iter()
                            .all(|request| !coverage.covers(arm.action, &request.action))
                    {
                        continue;
                    }
                    let finished = self
                        .inputs
                        .unit_effects
                        .get(&arm.fact.unit)
                        .and_then(|summary| summary.exits.as_ref())
                        .and_then(crate::CallableExitSummary::handler_finish_sources);
                    match finished {
                        Some(finished) => sources.extend(self.returned_callable_bindings(
                            &finished,
                            &Default::default(),
                            state,
                            expr,
                            None,
                        )?),
                        None => {
                            sources.insert(CallableValueOrigin::Unavailable { expr: handler });
                        }
                    }
                }
            }
            _ => {
                sources.insert(CallableValueOrigin::Unavailable { expr: handler });
            }
        }
        if sources.is_empty() {
            sources.insert(CallableValueOrigin::Unavailable { expr });
        }
        state
            .local_latent_values
            .record_value_sources(expr, sources);
        Ok(())
    }
}

fn handler_arm_units_match(
    owner: HirItemId,
    handlers: &[etas_hir::HirHandlerArmId],
    units: impl IntoIterator<Item = EffectUnit>,
) -> bool {
    handlers
        .iter()
        .map(|arm| EffectUnit::HandlerArm { owner, arm: *arm })
        .eq(units)
}

fn may_avoid(
    trace: &ActionTraceDomain,
    blocks: &impl Fn(&crate::Effect) -> bool,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<bool, EffectSpecializationError> {
    enum Work<'a> {
        Trace(&'a ActionTraceDomain),
        Join { count: usize, sequence: bool },
    }
    let mut pending = vec![Work::Trace(trace)];
    let mut values = Vec::new();
    while let Some(work) = pending.pop() {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectSpecializationError::Interrupted)?;
        }
        match work {
            Work::Trace(ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts))
                if !parts.is_empty() =>
            {
                pending.push(Work::Join {
                    count: parts.len(),
                    sequence: matches!(work, Work::Trace(ActionTraceDomain::Seq(_))),
                });
                pending.extend(parts.iter().rev().map(Work::Trace));
            }
            Work::Trace(ActionTraceDomain::Event(event)) => values.push(!blocks(&event.action)),
            // Widened/may-action footprints, symbolic calls and zero repetitions
            // do not prove that any particular handler is entered.
            Work::Trace(_) => values.push(true),
            Work::Join { count, sequence } => {
                let start = values.len().checked_sub(count).ok_or_else(|| {
                    EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "action trace completion fold has missing children".into(),
                    }
                })?;
                let result = if sequence {
                    values[start..].iter().all(|value| *value)
                } else {
                    values[start..].iter().any(|value| *value)
                };
                values.truncate(start);
                values.push(result);
            }
        }
    }
    if values.len() != 1 {
        return Err(EffectSpecializationError::InvalidCallableEnvironment {
            reason: "action trace completion fold has no unique root".into(),
        });
    }
    Ok(values[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ordered_must_events_prove_that_a_finishing_handler_is_entered() {
        let action = crate::Effect::Error(etas_types::TypeId(1));
        let event = ActionTraceDomain::Event(crate::ActionEvent {
            action: action.clone(),
            span: etas_core::Span::empty(etas_core::SourceId(1), etas_core::TextSize(0)),
            source: crate::ActionEventSource::Perform,
        });
        let blocks = |effect: &crate::Effect| *effect == action;
        assert!(!may_avoid(&event, &blocks, None).unwrap());
        assert!(
            may_avoid(
                &ActionTraceDomain::Choice(vec![event.clone(), ActionTraceDomain::Empty]),
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(
            !may_avoid(
                &ActionTraceDomain::Seq(vec![
                    ActionTraceDomain::Choice(vec![event.clone(), ActionTraceDomain::Empty]),
                    event.clone()
                ]),
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(may_avoid(&ActionTraceDomain::Repeat(Box::new(event)), &blocks, None).unwrap());
        assert!(
            may_avoid(
                &ActionTraceDomain::UnknownOrder(crate::EffectSet::one(action.clone())),
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(
            may_avoid(
                &ActionTraceDomain::Widened {
                    actions: crate::EffectSet::one(action.clone()),
                    parameter_calls: vec![]
                },
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(may_avoid(&ActionTraceDomain::Choice(vec![]), &blocks, None).unwrap());
    }

    #[test]
    fn handler_arm_evidence_requires_exact_owner_and_order() {
        let owner = HirItemId(1);
        let arms = [etas_hir::HirHandlerArmId(2), etas_hir::HirHandlerArmId(3)];
        let units = arms.map(|arm| EffectUnit::HandlerArm { owner, arm });
        assert!(handler_arm_units_match(owner, &arms, units));
        assert!(!handler_arm_units_match(owner, &arms, [units[1], units[0]]));
        assert!(!handler_arm_units_match(owner, &arms, [units[0]]));
        assert!(!handler_arm_units_match(HirItemId(4), &arms, units));
    }

    #[test]
    fn handler_arm_evidence_rejects_missing_action_and_wrong_unit() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
            "effect Gate { action ask() -> bool; } flow run() -> bool { return handle { perform Gate.ask() } with { Gate.ask() => { resume true; } }; }",
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let std = etas_std::standard_registry();
        let registry = crate::EffectRegistry::from_hir_types_dependencies_and_std(
            &hir,
            &types,
            &Default::default(),
            &std,
        )
        .unwrap();
        let output = crate::RunEffectPipeline::run(crate::EffectPipelineInput {
            frozen_calls: None,
            hir: &hir,
            types: &types,
            std_registry: &std,
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
        let mut semantics = EffectSemantics::new(&hir, &types, &std, &registry, &[], &[]);
        semantics.inputs.handler_values = output.facts.handler_values;
        semantics.inputs.handler_action_effects = output.facts.handler_action_effects;
        let handler = *semantics.inputs.handler_values.keys().next().unwrap();
        let fact = &semantics.inputs.handler_values[&handler];
        let EffectUnit::HandlerArm { owner, arm } = fact.arms[0].unit else {
            panic!("handler arm unit")
        };
        assert_eq!(
            semantics
                .checked_handler_arms(handler, Some(owner))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        let action = semantics
            .inputs
            .handler_action_effects
            .remove(&arm)
            .unwrap();
        assert!(matches!(
            semantics.checked_handler_arms(handler, Some(owner)),
            Err(EffectSpecializationError::InvalidCallableEnvironment { .. })
        ));
        semantics.inputs.handler_action_effects.insert(arm, action);
        semantics
            .inputs
            .handler_values
            .get_mut(&handler)
            .unwrap()
            .arms[0]
            .unit = EffectUnit::Item(owner);
        assert!(matches!(
            semantics.checked_handler_arms(handler, Some(owner)),
            Err(EffectSpecializationError::InvalidCallableEnvironment { .. })
        ));
    }

    #[test]
    fn trace_completion_analysis_propagates_cancellation() {
        struct Stop;
        impl etas_utils::PipelineExecutionControl for Stop {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel trace completion analysis".into(),
                })
            }
        }
        assert!(matches!(
            may_avoid(&ActionTraceDomain::Empty, &|_| false, Some(&Stop)),
            Err(EffectSpecializationError::Interrupted(interruption))
                if interruption.kind == etas_utils::PipelineInterruptionKind::Cancelled
        ));
    }
}
