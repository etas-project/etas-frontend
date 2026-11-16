use std::collections::{BTreeMap, BTreeSet};

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::HirItemId;

use crate::{
    ActionTraceDomain, Effect, EffectCoverage, EffectOutput, EffectPipelineError, EffectRegistry,
    EffectRow, EffectSet, EffectUnit, ExternalEffectSummaryMetadata, FrontendRejectionReason,
    InterpreterSupport,
};

use super::analysis::TraceSpecAnalysisOutput;
use super::analysis::{
    ClauseOutcome, DenialStaticDisposition, TemporalStaticDisposition, TemporalUncertainty,
    analyze_denial,
};
use super::model::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecModelStore};
use super::monitor;
use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic, resolve_anchor};

pub struct TraceSpecValidationInput<'a> {
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeOutput,
    pub registry: &'a EffectRegistry,
    pub models: &'a TraceSpecModelStore,
    pub analysis: &'a TraceSpecAnalysisOutput,
    pub dispositions: &'a super::TraceStaticDispositions,
    /// Plans verified against `analysis`. A conformance is discharged as a
    /// residual requirement only through one of these plans.
    pub residual_plans: &'a crate::residual::VerifiedResidualPlans,
    pub external_summaries: &'a [crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
    pub external_implementations:
        &'a [crate::AnchoredExternalMetadata<crate::ExternalSpecImplementationMetadata>],
    pub reachable_items: Option<&'a BTreeSet<HirItemId>>,
    pub control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
}

pub fn validate_trace_spec_facts(
    effects: &mut EffectOutput,
    input: TraceSpecValidationInput<'_>,
) -> Result<(), EffectPipelineError> {
    let TraceSpecValidationInput {
        hir,
        types,
        registry,
        models,
        analysis,
        dispositions,
        residual_plans,
        external_summaries: _,
        external_implementations,
        reachable_items,
        control,
    } = input;
    analysis.validate_source_conformances(models, types, effects)?;
    validate_item_policies(
        hir,
        TraceSpecEnv { types, registry },
        effects,
        models,
        (analysis, dispositions, residual_plans),
        reachable_items,
        control,
    )?;
    reject_uncovered_residuals(hir, effects, residual_plans, reachable_items)?;
    for (identity, alternatives) in &models.external_referenced_methods {
        validate_external_method_contract(
            types,
            registry,
            effects,
            identity,
            alternatives,
            external_implementations,
        )?;
    }
    validate_external_item_policies(types, registry, effects, models, analysis)
}

/// Own and propagated obligation instances must have exact coverage in this
/// run's verified plans. Propagation does not change the callee scope owner or
/// install that scope at a consumer expression. Unmapped metadata still rejects.
/// Selected-target support is a separate project-level gate.
fn reject_uncovered_residuals(
    hir: &etas_hir::HirRevision,
    effects: &mut EffectOutput,
    residual_plans: &crate::residual::VerifiedResidualPlans,
    reachable_items: Option<&BTreeSet<HirItemId>>,
) -> Result<(), EffectPipelineError> {
    let verified = residual_plans
        .plans()
        .flat_map(|plan| plan.plan().obligations().iter())
        .map(|obligation| (obligation.id(), obligation))
        .collect::<BTreeMap<_, _>>();
    let covered = |checks: &crate::ResidualCheckSet| {
        checks.iter().all(|reference| {
            verified
                .get(&reference.obligation())
                .is_some_and(|obligation| obligation.accepts_reference(reference))
        })
    };
    let mut anchors = BTreeMap::<DiagnosticAnchor, Option<HirItemId>>::new();
    let mut queue =
        |anchor: DiagnosticAnchor, owner: Option<HirItemId>| -> Result<(), EffectPipelineError> {
            if let Some(items) = reachable_items {
                let item = owner.ok_or_else(|| EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!("residual owner for {anchor:?}"),
                })?;
                if !items.contains(&item) {
                    return Ok(());
                }
            }
            if let Some(previous) = anchors.insert(anchor.clone(), owner)
                && previous != owner
            {
                return Err(EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!("conflicting residual owners for {anchor:?}"),
                });
            }
            Ok(())
        };
    for (unit, summary) in &effects.facts.unit_effects {
        if !summary.residual_checks.is_empty() {
            let owner = checked_unit_owner(hir, *unit)?;
            if !covered(&summary.residual_checks) {
                queue(DiagnosticAnchor::Unit(*unit), Some(owner))?;
            }
        }
    }
    for (item, summary) in &effects.facts.item_effects {
        if !summary.residual_checks.is_empty()
            && (!covered(&summary.residual_checks)
                || !residual_plans
                    .item_references(*item)
                    .is_subset(&summary.residual_checks.iter().collect()))
        {
            checked_unit_owner(hir, EffectUnit::Item(*item))?;
            queue(DiagnosticAnchor::Unit(EffectUnit::Item(*item)), Some(*item))?;
        }
    }
    for (expr, summary) in &effects.facts.expr_effects {
        if !summary.residual_checks.is_empty() && !covered(&summary.residual_checks) {
            queue(
                DiagnosticAnchor::Expr(*expr),
                hir.index().expr_item.get(expr).copied(),
            )?;
        }
    }
    for (stmt, summary) in &effects.facts.stmt_effects {
        if !summary.residual_checks.is_empty() && !covered(&summary.residual_checks) {
            queue(
                DiagnosticAnchor::Stmt(*stmt),
                hir.index().stmt_item.get(stmt).copied(),
            )?;
        }
    }
    for (expr, fact) in &effects.facts.latent_effects {
        if !fact.summary.residual_checks.is_empty() {
            if fact.value != *expr {
                return Err(EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!("inconsistent latent residual {expr:?}"),
                });
            }
            if !covered(&fact.summary.residual_checks) {
                queue(
                    DiagnosticAnchor::Expr(*expr),
                    hir.index().expr_item.get(expr).copied(),
                )?;
            }
        }
    }
    for (expr, fact) in &effects.facts.std_callable_values {
        if !fact.summary.residual_checks.is_empty() {
            if fact.value != *expr {
                return Err(EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!("inconsistent callable residual {expr:?}"),
                });
            }
            if !covered(&fact.summary.residual_checks) {
                queue(
                    DiagnosticAnchor::Expr(*expr),
                    hir.index().expr_item.get(expr).copied(),
                )?;
            }
        }
    }
    for (expr, fact) in &effects.facts.performed_actions {
        if !fact.summary.residual_checks.is_empty() {
            if fact.expr != *expr {
                return Err(EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!("inconsistent action residual {expr:?}"),
                });
            }
            if !covered(&fact.summary.residual_checks) {
                queue(
                    DiagnosticAnchor::Expr(*expr),
                    hir.index().expr_item.get(expr).copied(),
                )?;
            }
        }
    }
    for (reference, analysis) in &effects.facts.callable_references {
        if let crate::facts::CallableReferenceAnalysis::Solved { summary, .. } = analysis
            && !summary.residual_checks.is_empty()
        {
            if hir.index().expr_item.get(&reference.value).copied() != Some(reference.owner) {
                return Err(EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!(
                        "inconsistent callable reference residual {:?}",
                        reference.value
                    ),
                });
            }
            if !covered(&summary.residual_checks) {
                queue(
                    DiagnosticAnchor::Expr(reference.value),
                    Some(reference.owner),
                )?;
            }
        }
    }
    for (captured, analysis) in &effects.facts.captured_callables {
        if let crate::facts::CapturedCallableAnalysis::Solved(summary) = analysis
            && !summary.residual_checks.is_empty()
        {
            let owner = checked_unit_owner(hir, captured.unit)?;
            if !covered(&summary.residual_checks) {
                queue(DiagnosticAnchor::Unit(captured.unit), Some(owner))?;
            }
        }
    }
    for contract in &effects.facts.public_contracts {
        if !contract.residual_checks.is_empty() {
            checked_unit_owner(hir, EffectUnit::Item(contract.item))?;
            if !covered(&contract.residual_checks) {
                queue(
                    DiagnosticAnchor::Unit(EffectUnit::Item(contract.item)),
                    Some(contract.item),
                )?;
            }
        }
    }
    let symbols = effects
        .facts
        .symbol_effects
        .iter()
        .chain(&effects.facts.inferred_parameter_effects)
        .filter(|(_, summary)| {
            !summary.residual_checks.is_empty() && !covered(&summary.residual_checks)
        })
        .map(|(symbol, _)| *symbol)
        .collect::<BTreeSet<_>>();
    if !symbols.is_empty() {
        let mut symbol_owners = BTreeMap::new();
        for (item, data) in hir.items.iter() {
            if let Some(symbol) = data.symbol()
                && symbols.contains(&symbol)
                && symbol_owners
                    .insert(symbol, item)
                    .is_some_and(|previous| previous != item)
            {
                return Err(EffectPipelineError::MissingDiagnosticAnchor {
                    artifact: format!("ambiguous residual symbol owner {symbol:?}"),
                });
            }
        }
        for symbol in symbols {
            queue(
                DiagnosticAnchor::Symbol(symbol),
                symbol_owners.get(&symbol).copied(),
            )?;
        }
    }
    for (source, analysis) in &effects.facts.metadata_callables {
        if let crate::facts::MetadataCallableAnalysis::Solved { summary, .. } = analysis
            && !summary.residual_checks.is_empty()
        {
            if let Some(environment) = source.environment {
                let owner = effects
                    .facts
                    .callable_environments
                    .get(&environment)
                    .map(|env| env.owner)
                    .ok_or_else(|| EffectPipelineError::MissingDiagnosticAnchor {
                        artifact: format!("external callable residual environment {environment:?}"),
                    })?;
                checked_unit_owner(hir, EffectUnit::Item(owner))?;
                if reachable_items.is_some_and(|items| !items.contains(&owner)) {
                    continue;
                }
            }
            return Err(EffectPipelineError::MissingDiagnosticAnchor {
                artifact: format!("external callable residual {source:?}"),
            });
        }
    }
    let diagnostics = anchors
        .keys()
        .map(|anchor| {
            materialize_effect_diagnostic(
                hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                anchor.clone(),
                "residual checks are not covered by verified residual plans",
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    for owner in anchors.values().flatten().copied().collect::<BTreeSet<_>>() {
        reject_item_for_trace_spec(effects, owner);
    }
    effects.diagnostics.extend(diagnostics);
    Ok(())
}

fn checked_unit_owner(
    hir: &etas_hir::HirRevision,
    unit: EffectUnit,
) -> Result<HirItemId, EffectPipelineError> {
    let (expected, actual) = match unit {
        EffectUnit::Item(item) => (item, hir.items.get(item).map(|_| item)),
        EffectUnit::AnonymousFlow { owner, value, .. } => {
            (owner, hir.index().expr_item.get(&value).copied())
        }
        EffectUnit::HandlerArm { owner, arm } => {
            let actual = hir
                .index()
                .handler_arm_owner
                .get(&arm)
                .and_then(|owner| crate::validate::owner_item(&hir.view(), *owner));
            (owner, actual)
        }
    };
    if actual != Some(expected) {
        return Err(EffectPipelineError::MissingDiagnosticAnchor {
            artifact: format!("inconsistent residual unit owner {unit:?}"),
        });
    }
    Ok(expected)
}

fn validate_item_policies(
    hir: &etas_hir::HirProgram,
    env: TraceSpecEnv<'_>,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    analyses: (
        &TraceSpecAnalysisOutput,
        &super::TraceStaticDispositions,
        &crate::residual::VerifiedResidualPlans,
    ),
    reachable_items: Option<&BTreeSet<HirItemId>>,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(), EffectPipelineError> {
    let TraceSpecEnv { types, registry } = env;
    let (analysis, static_dispositions, residual_plans) = analyses;
    for (item, alternatives) in &models.referenced_by_item {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        if reachable_items.is_some_and(|items| !items.contains(item)) {
            continue;
        }
        let Some(trace_spec_summary) = analysis.summaries.get(EffectUnit::Item(*item)) else {
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Unit(EffectUnit::Item(*item)),
                "trace spec validation requires solved trace spec summary facts",
            )?);
            reject_item_for_trace_spec(effects, *item);
            continue;
        };
        if trace_spec_summary.incomplete {
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Unit(EffectUnit::Item(*item)),
                "trace spec validation requires complete trace spec summary facts",
            )?);
            reject_item_for_trace_spec(effects, *item);
            continue;
        }
        let action_trace = &trace_spec_summary.action_trace;
        let requested_actions = &trace_spec_summary.requested_actions;
        let coverage = EffectCoverage {
            registry,
            types: &types.store,
        };
        if action_trace_agrees(&coverage, action_trace, &requested_actions.effects) != Some(true) {
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Unit(EffectUnit::Item(*item)),
                "trace spec action trace and requested actions disagree or exceed the validation work limit",
            )?);
            reject_item_for_trace_spec(effects, *item);
            continue;
        }
        let Some(item_effect_summary) = effects.facts.item_effects.get(item) else {
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Unit(EffectUnit::Item(*item)),
                "trace spec validation requires materialized item effect facts",
            )?);
            reject_item_for_trace_spec(effects, *item);
            continue;
        };
        let allow_requested_actions = EffectCoverage {
            registry,
            types: &types.store,
        }
        .subtract_handled(requested_actions, &item_effect_summary.default_actions);
        let fallback_span = resolve_anchor(hir, &DiagnosticAnchor::Unit(EffectUnit::Item(*item)))
            .ok_or_else(|| EffectPipelineError::MissingDiagnosticAnchor {
            artifact: format!("trace spec item {item:?}"),
        })?;
        let Some(outcomes) = analysis.clause_outcomes.get(item) else {
            effects.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                fallback_span,
                "trace spec validation requires temporal analysis facts",
            ));
            reject_item_for_trace_spec(effects, *item);
            continue;
        };
        let Some(permission_outcomes) = analysis.permission_outcomes.get(item) else {
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Unit(EffectUnit::Item(*item)),
                "trace spec validation requires permission analysis facts",
            )?);
            reject_item_for_trace_spec(effects, *item);
            continue;
        };
        let indices = models.referenced_fact_indices.get(item).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace conformance validation has no checked type fact indices".into(),
            }
        })?;
        if indices.len() != alternatives.len() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace conformance validation has inconsistent type fact indices".into(),
            });
        }
        let dispositions = indices
            .iter()
            .map(|index| static_dispositions.conformance(*index, *item, analysis, control))
            .collect::<Result<Vec<_>, _>>();
        let dispositions = match dispositions {
            Ok(value) => value,
            Err(EffectPipelineError::InvalidTraceSpecSourceSites { reason }) => {
                effects.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    fallback_span,
                    format!("trace spec analysis is incomplete or inconsistent: {reason}"),
                ));
                reject_item_for_trace_spec(effects, *item);
                continue;
            }
            Err(error) => return Err(error),
        };
        let residual = indices
            .iter()
            .map(|index| {
                residual_plans
                    .plan(*index)
                    .is_some_and(|plan| plan.item() == *item)
            })
            .collect::<Vec<_>>();
        let diagnostics = validate_conformances_with_residuals(
            env,
            alternatives,
            action_trace,
            requested_actions,
            &allow_requested_actions,
            fallback_span,
            (
                Some(outcomes),
                Some(permission_outcomes),
                Some(&dispositions),
            ),
            &residual,
        );
        for mut diagnostic in diagnostics {
            for index in indices {
                if let Some(failure) = residual_plans.failure(*index) {
                    diagnostic
                        .notes
                        .push(format!("residual monitor preparation failed: {failure:?}"));
                }
            }
            effects.diagnostics.push(diagnostic);
            reject_item_for_trace_spec(effects, *item);
        }
        if residual.iter().any(|value| *value)
            && let Some(summary) = effects.facts.item_effects.get_mut(item)
        {
            // Keep the identified obligations in the owner's summary so
            // every later consumer sees an outstanding residual requirement.
            for reference in residual_plans.item_references(*item) {
                summary.residual_checks.insert(reference.clone());
            }
        }
        if let Some(summary) = effects.facts.item_effects.get_mut(item)
            && let Some(requirements) = effects.facts.requirements.items.get(item)
        {
            summary.trace_spec_obligations.union_assign(requirements);
        }
        if !matches!(
            hir.items.get(*item),
            Some(
                etas_hir::HirItem::Flow(_)
                    | etas_hir::HirItem::Tool(_)
                    | etas_hir::HirItem::Agent(etas_hir::HirAgentDecl {
                        body: etas_hir::HirAgentBody::Source { .. },
                        ..
                    })
            )
        ) {
            reject_item_for_trace_spec(effects, *item);
        }
    }
    Ok(())
}

pub(super) fn validate_external_item_policies(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    analysis: &TraceSpecAnalysisOutput,
) -> Result<(), EffectPipelineError> {
    for (item, alternatives) in &models.external_referenced_by_item {
        let Some(outcome) = analysis.external_outcomes.get(item) else {
            return Err(EffectPipelineError::InvalidExternalMetadata {
                package: item.package.to_string(),
                reason: format!(
                    "trace spec conformance for `{}` has no external analysis result",
                    item.path.join(".")
                ),
            });
        };
        let super::analysis::ExternalTraceOutcome::Ready {
            span,
            trace,
            clauses,
            permissions,
        } = outcome
        else {
            continue;
        };
        for diagnostic in validate_conformances(
            TraceSpecEnv { types, registry },
            alternatives,
            &trace.action_trace,
            &trace.requested_actions,
            &trace.allow_requested_actions,
            *span,
            (Some(clauses), Some(permissions), None),
        ) {
            effects.diagnostics.push(diagnostic);
        }
    }
    Ok(())
}

fn validate_external_method_contract(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    identity: &etas_code::MethodId,
    alternatives: &super::model::TraceSpecConformances,
    implementations: &[crate::AnchoredExternalMetadata<
        crate::ExternalSpecImplementationMetadata,
    >],
) -> Result<(), EffectPipelineError> {
    let invalid = |reason: &str| EffectPipelineError::InvalidExternalMetadata {
        package: format!("{:?}", identity.implementation().package()),
        reason: reason.into(),
    };
    let mut methods = implementations
        .iter()
        .filter(|method| &method.identity == identity);
    let (Some(method), None) = (methods.next(), methods.next()) else {
        return Err(invalid(
            "method conformance requires one anchored implementation",
        ));
    };
    let [etas_package_metadata::CallableValueSource::Value(value)] =
        method.graph.sources.as_slice()
    else {
        return Err(invalid(
            "method conformance requires an owned callable graph root",
        ));
    };
    let source = crate::MetadataCallableSource {
        analysis: crate::CallableAnalysisKind::CheckedRevision,
        graph: method.graph_key,
        value: *value,
        environment: None,
    };
    let Some(crate::facts::MetadataCallableAnalysis::Solved { summary, .. }) =
        effects.facts.metadata_callables.get(&source)
    else {
        return Err(invalid(
            "method conformance requires complete checked callable analysis",
        ));
    };
    let allowed = EffectCoverage {
        registry,
        types: &types.store,
    }
    .subtract_handled(&summary.requested_actions, &summary.default_actions);
    let diagnostics = validate_conformances(
        TraceSpecEnv { types, registry },
        alternatives,
        &summary.action_trace,
        &summary.requested_actions,
        &allowed,
        method.span,
        (None, None, None),
    );
    effects.diagnostics.extend(diagnostics);
    Ok(())
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub(super) struct ExternalTraceSummary {
    pub(super) action_trace: ActionTraceDomain,
    pub(super) requested_actions: EffectRow,
    pub(super) allow_requested_actions: EffectRow,
}

pub(super) fn trace_summary_from_external_metadata(
    summary: &ExternalEffectSummaryMetadata,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    external_span: Span,
) -> Option<ExternalTraceSummary> {
    if summary.requested_actions.tail.is_some() || summary.handled_requested_actions.tail.is_some()
    {
        effects.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            external_span,
            format!(
                "external effect summary `{}` has an unresolved action row",
                summary.item.join(".")
            ),
        ));
        return None;
    }
    let parameters = summary
        .param_names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            Some((
                name.clone(),
                crate::ParameterCallTarget::Interface(u32::try_from(index).ok()?),
            ))
        })
        .collect::<Option<Vec<_>>>();
    let action_trace = parameters.as_ref().and_then(|parameters| {
        crate::external_trace::lower_external_action_trace(
            &summary.action_trace,
            parameters,
            external_span,
            |action| crate::external_trace::lower_external_action(registry, &types.store, action),
        )
    });
    let Some(action_trace) = action_trace else {
        effects.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            external_span,
            format!(
                "external effect summary `{}` has invalid or unresolved action trace",
                summary.item.join(".")
            ),
        ));
        return None;
    };
    let mut requested_actions = Vec::new();
    let mut handled_actions = Vec::new();
    let mut handled_effects = BTreeSet::new();
    for handled in &summary.handled_requested_actions.effects {
        let Some(effect) =
            crate::external_trace::lower_external_action(registry, &types.store, handled)
        else {
            effects.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                external_span,
                format!(
                    "external effect summary `{}` has unresolved handled action `{}`",
                    summary.item.join("."),
                    handled.path.join(".")
                ),
            ));
            return None;
        };
        if !effect.is_action() {
            effects.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                external_span,
                format!(
                    "external effect summary `{}` handled effect `{}` is not an action",
                    summary.item.join("."),
                    handled.path.join(".")
                ),
            ));
            return None;
        }
        handled_effects.insert(effect);
    }
    for action in &summary.requested_actions.effects {
        let Some(effect) =
            crate::external_trace::lower_external_action(registry, &types.store, action)
        else {
            effects.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                external_span,
                format!(
                    "external effect summary `{}` has unresolved requested action `{}`",
                    summary.item.join("."),
                    action.path.join(".")
                ),
            ));
            return None;
        };
        if !effect.is_action() {
            effects.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                external_span,
                format!(
                    "external effect summary `{}` requested effect `{}` is not an action",
                    summary.item.join("."),
                    action.path.join(".")
                ),
            ));
            return None;
        }
        requested_actions.push(effect.clone());
        if handled_effects.contains(&effect) {
            handled_actions.push(effect);
        }
    }
    let requested_actions = EffectRow::closed(EffectSet::from_iter(requested_actions));
    let coverage = EffectCoverage {
        registry,
        types: &types.store,
    };
    let agreement = action_trace_agrees(&coverage, &action_trace, &requested_actions.effects);
    if agreement != Some(true) {
        effects.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            external_span,
            format!(
                "external effect summary `{}` action trace and requested actions {}",
                summary.item.join("."),
                if agreement.is_none() {
                    "exceed the validation work limit"
                } else {
                    "do not agree"
                },
            ),
        ));
        return None;
    }
    let handled_actions = EffectRow::closed(EffectSet::from_iter(handled_actions));
    let allow_requested_actions = EffectCoverage {
        registry,
        types: &types.store,
    }
    .subtract_handled(&requested_actions, &handled_actions);
    Some(ExternalTraceSummary {
        action_trace,
        requested_actions,
        allow_requested_actions,
    })
}

fn action_trace_agrees(
    coverage: &EffectCoverage<'_>,
    action_trace: &ActionTraceDomain,
    requested_actions: &EffectSet,
) -> Option<bool> {
    let trace_actions = action_trace.action_set();
    let mut remaining = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    action_set_covers(coverage, &trace_actions, requested_actions, &mut remaining).and_then(
        |forward| {
            if !forward {
                return Some(false);
            }
            action_set_covers(coverage, requested_actions, &trace_actions, &mut remaining)
        },
    )
}

fn action_set_covers(
    coverage: &EffectCoverage<'_>,
    covering: &EffectSet,
    required: &EffectSet,
    remaining: &mut usize,
) -> Option<bool> {
    for action in required.iter() {
        let mut covered = false;
        for candidate in covering.iter() {
            *remaining = remaining.checked_sub(1)?;
            if coverage.covers(candidate, action) {
                covered = true;
                break;
            }
        }
        if !covered {
            return Some(false);
        }
    }
    Some(true)
}

pub(super) fn external_summary_matches_item(
    summary: &ExternalEffectSummaryMetadata,
    item: &[String],
) -> bool {
    summary.item == item
        && summary
            .import_root
            .as_ref()
            .map(|root| module_path_has_import_root(item, root))
            .unwrap_or(true)
}

fn module_path_has_import_root(path: &[String], root: &str) -> bool {
    let root_segments = root
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    !root_segments.is_empty()
        && path.len() >= root_segments.len()
        && path
            .iter()
            .zip(root_segments)
            .all(|(segment, root_segment)| segment == root_segment)
}

#[derive(Clone, Copy)]
struct TraceSpecEnv<'a> {
    types: &'a etas_types::TypeOutput,
    registry: &'a EffectRegistry,
}

type ConformanceOutcomes<'a> = (
    Option<&'a super::analysis::ClauseConformanceOutcomes>,
    Option<&'a super::analysis::PermissionConformanceOutcomes>,
    Option<&'a [super::analysis::CheckedTraceStaticDisposition]>,
);

fn validate_conformances(
    env: TraceSpecEnv<'_>,
    conformances: &super::model::TraceSpecConformances,
    trace: &ActionTraceDomain,
    requested_actions: &EffectRow,
    allow_requested_actions: &EffectRow,
    span: Span,
    outcomes: ConformanceOutcomes<'_>,
) -> Vec<Diagnostic> {
    validate_conformances_with_residuals(
        env,
        conformances,
        trace,
        requested_actions,
        allow_requested_actions,
        span,
        outcomes,
        &[],
    )
}

/// `residual[index]` marks a conformance discharged by a verified residual
/// plan. Only an unproven (never a violated or inconsistent) conformance with
/// such a plan omits its static diagnostics.
#[allow(clippy::too_many_arguments)]
fn validate_conformances_with_residuals(
    env: TraceSpecEnv<'_>,
    conformances: &super::model::TraceSpecConformances,
    trace: &ActionTraceDomain,
    requested_actions: &EffectRow,
    allow_requested_actions: &EffectRow,
    span: Span,
    outcomes: ConformanceOutcomes<'_>,
    residual: &[bool],
) -> Vec<Diagnostic> {
    let (clause_outcomes, permission_outcomes, dispositions) = outcomes;
    if !residual.is_empty() && (residual.len() != conformances.len() || dispositions.is_none()) {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            "trace spec residual plans do not match the checked conformances",
        )];
    }
    if conformances.is_empty() {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            "trace spec conformance has no checked model",
        )];
    }
    if clause_outcomes.is_some_and(|outcomes| outcomes.len() != conformances.len())
        || permission_outcomes.is_some_and(|outcomes| outcomes.len() != conformances.len())
        || dispositions.is_some_and(|outcomes| outcomes.len() != conformances.len())
    {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            "trace spec conformance and temporal analysis facts disagree",
        )];
    }
    conformances
        .iter()
        .enumerate()
        .flat_map(|(index, alternatives)| {
            let diagnostics = validate_alternatives(
                env,
                alternatives,
                trace,
                requested_actions,
                allow_requested_actions,
                span,
                (
                    clause_outcomes.map(|outcomes| outcomes[index].as_slice()),
                    permission_outcomes.map(|outcomes| outcomes[index].as_slice()),
                ),
            );
            if let Some(expected) = dispositions {
                use super::analysis::CheckedTraceStaticDisposition as Disposition;
                let actual = if diagnostics.is_empty() {
                    Disposition::Proven
                } else if diagnostics.iter().any(is_definite_trace_violation) {
                    Disposition::Violated
                } else {
                    Disposition::Unsupported
                };
                if actual != expected[index] {
                    return vec![Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        span,
                        "trace conformance disposition disagrees with validation",
                    )];
                }
                if actual == Disposition::Unsupported && residual.get(index) == Some(&true) {
                    return Vec::new();
                }
            }
            diagnostics
        })
        .collect()
}

type AlternativeAnalysis<'a> = (
    Option<&'a [Vec<Option<ClauseOutcome>>]>,
    Option<&'a [Option<super::analysis::PermissionOutcome>]>,
);

fn validate_alternatives(
    env: TraceSpecEnv<'_>,
    alternatives: &TraceSpecClauseAlternatives,
    trace: &ActionTraceDomain,
    requested_actions: &EffectRow,
    allow_requested_actions: &EffectRow,
    fallback_span: Span,
    outcomes: AlternativeAnalysis<'_>,
) -> Vec<Diagnostic> {
    let (clause_outcomes, permission_outcomes) = outcomes;
    if alternatives.is_empty() || alternatives.iter().any(Vec::is_empty) {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec conformance contains an empty alternative model",
        )];
    }
    if permission_outcomes.is_some_and(|outcomes| outcomes.len() != alternatives.len())
        || clause_outcomes.is_some_and(|outcomes| {
            outcomes.len() != alternatives.len()
                || alternatives.iter().zip(outcomes).any(|(clauses, results)| {
                    clauses.len() != results.len()
                        || clauses.iter().zip(results).any(|(clause, result)| {
                            matches!(
                                clause,
                                TraceSpecClause::Deny { .. }
                                    | TraceSpecClause::RequireBefore { .. }
                                    | TraceSpecClause::RequireAfter { .. }
                            ) != result.is_some()
                                || result
                                    .as_ref()
                                    .is_some_and(|outcome| !outcome.matches_clause(clause))
                                || matches!(result, Some(ClauseOutcome::Denial(outcome))
                                if outcome.requested.as_ref() != requested_actions
                                    || outcome.unresolved_calls != trace.has_parameter_calls())
                        })
                })
        })
    {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec alternatives and temporal analysis facts disagree",
        )];
    }
    let mut first_violation = None;
    let mut first_unproven = None;
    for (index, clauses) in alternatives.iter().enumerate() {
        let diagnostics = validate_clauses(
            env,
            clauses,
            trace,
            requested_actions,
            allow_requested_actions,
            fallback_span,
            (
                clause_outcomes.map(|outcomes| outcomes[index].as_slice()),
                permission_outcomes.map(|outcomes| outcomes[index].as_ref()),
            ),
        );
        if diagnostics.is_empty() {
            return Vec::new();
        }
        let has_violation = diagnostics.iter().any(is_definite_trace_violation);
        if has_violation {
            if first_violation.is_none() {
                first_violation = Some(
                    diagnostics
                        .into_iter()
                        .filter(is_definite_trace_violation)
                        .collect(),
                );
            }
        } else if first_unproven.is_none() {
            first_unproven = Some(diagnostics);
        }
    }
    first_unproven.or(first_violation).unwrap_or_else(|| {
        vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec alternatives produced no checked disposition",
        )]
    })
}

fn is_definite_trace_violation(diagnostic: &Diagnostic) -> bool {
    matches!(
        diagnostic.code,
        etas_core::DiagnosticCode::Effect(
            EffectDiagnosticCode::TraceSpecDenied
                | EffectDiagnosticCode::TraceSpecAllowViolation
                | EffectDiagnosticCode::TraceSpecRequirementNotDominating
        )
    )
}

fn validate_clauses(
    env: TraceSpecEnv<'_>,
    clauses: &[TraceSpecClause],
    trace: &ActionTraceDomain,
    requested_actions: &EffectRow,
    allow_requested_actions: &EffectRow,
    fallback_span: Span,
    outcomes: (
        Option<&[Option<ClauseOutcome>]>,
        Option<Option<&super::analysis::PermissionOutcome>>,
    ),
) -> Vec<Diagnostic> {
    let (clause_outcomes, permission_outcome) = outcomes;
    let TraceSpecEnv { types, registry } = env;
    if clauses.is_empty() {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec alternative contains no clauses",
        )];
    }
    let mut diagnostics = Vec::new();
    if clauses
        .iter()
        .any(|clause| matches!(clause, TraceSpecClause::Limit { .. }))
    {
        diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec limit requires a checked enforcement model",
        ));
    }
    let normalized = super::model::normalize_alternative(clauses);
    let Ok(normalized) = normalized else {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec clause normalization exceeds the artifact graph limit",
        )];
    };
    let allow = &normalized.permissions;
    let has_unresolved_calls = trace.has_parameter_calls();
    let fallback;
    let permission = match permission_outcome {
        Some(Some(outcome)) => Some(outcome),
        Some(None) => None,
        None => {
            fallback = super::analysis::analyze_permissions(
                clauses,
                allow,
                std::sync::Arc::new(allow_requested_actions.clone()),
                has_unresolved_calls,
                registry,
                types,
            );
            fallback.as_ref()
        }
    };
    let Some(permission) = permission.filter(|outcome| {
        outcome.requested.as_ref() == allow_requested_actions
            && outcome.unresolved_calls == has_unresolved_calls
            && outcome.permissions.len() == allow.len()
            && outcome.permissions.iter().zip(allow).all(
                |((actual, actual_origin, actual_clause), (expected, expected_origin))| {
                    actual == *expected
                        && actual_origin == expected_origin
                        && clauses
                            .get(expected_origin.0)
                            .is_some_and(|clause| clause.source_clause() == *actual_clause)
                },
            )
    }) else {
        return vec![Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec permission analysis is missing, inconsistent, or exceeds the work limit",
        )];
    };
    for (action, uncertain) in &permission.uncovered {
        let span = action_span(trace, action)
            .or_else(|| allow.first().map(|(_, (_, span))| *span))
            .unwrap_or(fallback_span);
        diagnostics.push(Diagnostic::effect_check(
            if *uncertain {
                EffectDiagnosticCode::IncompleteEffectFacts
            } else {
                EffectDiagnosticCode::TraceSpecAllowViolation
            },
            span,
            if *uncertain {
                "trace spec allow clauses cannot prove the requested action selector is covered"
            } else {
                "trace spec allow clauses do not cover a requested action"
            },
        ));
    }
    if permission.unresolved {
        diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            fallback_span,
            "trace spec allow clauses cannot prove coverage of unresolved requested actions",
        ));
    }
    for (pattern, (index, span)) in &normalized.denials {
        let disposition = if let Some(outcomes) = clause_outcomes {
            let Some(Some(ClauseOutcome::Denial(outcome))) = outcomes.get(*index) else {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    *span,
                    "trace spec deny clause is missing static analysis facts",
                ));
                continue;
            };
            outcome.disposition
        } else {
            analyze_denial(pattern, trace, requested_actions, registry, types)
        };
        match disposition {
            DenialStaticDisposition::Proven => {}
            DenialStaticDisposition::Violated => diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::TraceSpecDenied,
                *span,
                "trace spec deny clause rejects a requested action",
            )),
            DenialStaticDisposition::Unsupported => diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                *span,
                "trace spec deny clause cannot prove requested action selectors are disjoint",
            )),
        }
    }
    for prerequisite in &normalized.prerequisites {
        let (index, span) = prerequisite.origin;
        let disposition = if let Some(outcomes) = clause_outcomes {
            let Some(Some(ClauseOutcome::Temporal(result))) = outcomes.get(index) else {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "trace spec clause is missing temporal analysis facts",
                ));
                continue;
            };
            result.disposition
        } else {
            TemporalStaticDisposition::from_monitor(
                monitor::require_before(
                    trace,
                    prerequisite.guard,
                    prerequisite.target,
                    registry,
                    types,
                ),
                requested_actions.open.is_some(),
            )
        };
        let disposition = if requested_actions.open.is_some()
            && matches!(disposition, TemporalStaticDisposition::Proven)
        {
            TemporalStaticDisposition::Unsupported(TemporalUncertainty::OpenRequestedActions)
        } else {
            disposition
        };
        match disposition {
            TemporalStaticDisposition::Proven => {}
            TemporalStaticDisposition::Violated => diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::TraceSpecRequirementNotDominating,
                span,
                "trace spec prior-action obligation is not satisfied on every action path",
            )),
            TemporalStaticDisposition::Unsupported(TemporalUncertainty::OutcomeEvidence) => diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "trace spec prior-action analysis requires correlated accepted guard outcome evidence",
            )),
            TemporalStaticDisposition::Unsupported(TemporalUncertainty::OrderedActions | TemporalUncertainty::OpenRequestedActions) => diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "trace spec prior-action analysis needs ordered actions and correlated guard outcome facts",
            )),
        }
    }
    diagnostics
}

fn action_span(trace: &ActionTraceDomain, action: &Effect) -> Option<Span> {
    match trace {
        ActionTraceDomain::Event(event) if &event.action == action => Some(event.span),
        ActionTraceDomain::Seq(parts)
        | ActionTraceDomain::Choice(parts)
        | ActionTraceDomain::Parallel(parts) => {
            parts.iter().find_map(|part| action_span(part, action))
        }
        ActionTraceDomain::Repeat(inner)
        | ActionTraceDomain::TaskStart { body: inner, .. }
        | ActionTraceDomain::DefaultHandler { body: inner, .. } => action_span(inner, action),
        ActionTraceDomain::Empty
        | ActionTraceDomain::Event(_)
        | ActionTraceDomain::ParameterCall { .. }
        | ActionTraceDomain::TaskAwait { .. }
        | ActionTraceDomain::UnknownOrder(_)
        | ActionTraceDomain::Widened { .. } => None,
    }
}

pub(crate) fn reject_item_for_trace_spec(effects: &mut EffectOutput, item: etas_hir::HirItemId) {
    if let Some(summary) = effects.facts.item_effects.get_mut(&item) {
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::MissingRequirement);
        effects
            .facts
            .interpreter_support
            .items
            .insert(item, summary.support.clone());
    }
}

#[cfg(test)]
mod tests {
    use etas_core::{DiagnosticCode, EffectDiagnosticCode, SourceFile, SourceId};
    use etas_hir::{HirItem, lower_program};
    use etas_hir_analysis::interprocedural::SummaryStore;

    #[test]
    fn missing_later_or_permission_reports_incomplete_effect_facts() {
        let source = SourceFile::anonymous(
            "module app; effect Gate { action a() -> unit; action b() -> unit; } spec Safe: trace = +Gate.a | +Gate.b; flow main() -> unit ![] ~ Safe { return; }",
        );
        let parsed = etas_syntax::parse_program(source);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let std_registry = etas_std::standard_registry();
        let output = crate::RunEffectPipeline::run(crate::EffectPipelineInput {
            external_implementations: &[],
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
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
        .unwrap();
        assert!(
            output.effects.diagnostics.is_empty(),
            "{:?}",
            output.effects.diagnostics
        );
        let mut effects = crate::EffectOutput::default();
        let models = crate::trace_spec::materialize_trace_spec_models(
            &hir,
            &types,
            &output.artifacts.registry,
            &[],
            &[],
            &mut effects,
        )
        .unwrap();
        let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
        effects.facts = output.effects.facts;
        effects.facts.trace_specs = trace_specs;
        let mut analysis = crate::trace_spec::analyze_trace_spec_monitors(
            crate::trace_spec::TraceSpecAnalysisInput {
                source_snapshots: &[],
                external_trace_specs: &[],
                hir: &hir,
                types: &types,
                registry: &output.artifacts.registry,
                models: &models,
                units: &output.artifacts.units,
                external_summaries: &[],
            },
            &mut effects,
        )
        .unwrap();
        let (&item, conformances) = analysis.permission_outcomes.iter_mut().next().unwrap();
        conformances[0][1] = None;
        let dispositions =
            super::super::collect_trace_static_dispositions(&analysis, None, None).unwrap();
        super::validate_trace_spec_facts(
            &mut effects,
            super::TraceSpecValidationInput {
                external_summaries: &[],
                external_implementations: &[],
                hir: &hir,
                types: &types,
                registry: &output.artifacts.registry,
                models: &models,
                analysis: &analysis,
                dispositions: &dispositions,
                residual_plans: &Default::default(),
                reachable_items: None,
                control: None,
            },
        )
        .unwrap();
        assert!(effects.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.primary.span == hir.items[item].span()
        }));
        assert!(matches!(
            effects.facts.item_effects[&item].support,
            crate::InterpreterSupport::Rejected(_)
        ));
    }

    #[test]
    fn each_conformance_disposition_is_checked_even_when_another_fails() {
        let source = SourceFile::anonymous(
            "module app; effect Gate { action a() -> unit; } spec Allowed: trace = +Gate.a; spec Denied: trace = -Gate.a; flow main() -> unit ![Gate.a] ~ Allowed + Denied { perform Gate.a(); }",
        );
        let parsed = etas_syntax::parse_program(source);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let std_registry = etas_std::standard_registry();
        let output = crate::RunEffectPipeline::run(crate::EffectPipelineInput {
            external_implementations: &[],
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
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
        .unwrap();
        let mut effects = crate::EffectOutput::default();
        let models = crate::trace_spec::materialize_trace_spec_models(
            &hir,
            &types,
            &output.artifacts.registry,
            &[],
            &[],
            &mut effects,
        )
        .unwrap();
        let trace_specs = std::mem::take(&mut effects.facts.trace_specs);
        effects.facts = output.effects.facts;
        effects.facts.trace_specs = trace_specs;
        let analysis = crate::trace_spec::analyze_trace_spec_monitors(
            crate::trace_spec::TraceSpecAnalysisInput {
                source_snapshots: &[],
                external_trace_specs: &[],
                hir: &hir,
                types: &types,
                registry: &output.artifacts.registry,
                models: &models,
                units: &output.artifacts.units,
                external_summaries: &[],
            },
            &mut effects,
        )
        .unwrap();
        let (&item, conformances) = models.referenced_by_item.iter().next().unwrap();
        assert_eq!(conformances.len(), 2);
        let summary = analysis
            .summaries
            .get(crate::EffectUnit::Item(item))
            .unwrap();
        let allow = crate::EffectCoverage {
            registry: &output.artifacts.registry,
            types: &types.store,
        }
        .subtract_handled(
            &summary.requested_actions,
            &effects.facts.item_effects[&item].default_actions,
        );
        let forged = [
            crate::trace_spec::CheckedTraceStaticDisposition::Violated,
            crate::trace_spec::CheckedTraceStaticDisposition::Violated,
        ];
        let diagnostics = super::validate_conformances(
            super::TraceSpecEnv {
                types: &types,
                registry: &output.artifacts.registry,
            },
            conformances,
            &summary.action_trace,
            &summary.requested_actions,
            &allow,
            hir.items[item].span(),
            (
                Some(&analysis.clause_outcomes[&item]),
                Some(&analysis.permission_outcomes[&item]),
                Some(&forged),
            ),
        );
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains("disposition disagrees")
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)
        }));
    }

    use super::{
        TraceSpecEnv, TraceSpecModelStore, reject_uncovered_residuals,
        trace_summary_from_external_metadata, validate_alternatives, validate_clauses,
        validate_conformances, validate_item_policies,
    };
    use crate::trace_spec::{analysis::TraceSpecAnalysisOutput, domain::TraceSpecSummary};
    use crate::{EffectOutput, EffectRegistry, EffectSummary, EffectUnit};

    fn residual_summary() -> EffectSummary {
        use etas_code::{
            CodeFingerprint,
            residual::identity::{ObligationId, ResidualObligationRef},
        };
        let mut summary = EffectSummary::local();
        summary.residual_checks.insert(
            ResidualObligationRef::new(
                ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
                Default::default(),
            )
            .unwrap(),
        );
        summary
    }

    #[test]
    fn empty_alternative_models_never_become_successful_or_branches() {
        let source = SourceFile::new(SourceId(0), None, "spec".to_owned());
        let span = etas_core::Span::new(
            source.id,
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(4)),
        );
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let effect =
            crate::Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let row = crate::EffectRow::closed(crate::EffectSet::one(effect));
        let valid = vec![crate::trace_spec::TraceSpecClause::Deny {
            source_path: None,
            source_clause: None,
            pattern: crate::trace_spec::TraceSpecPattern {
                row: row.clone(),
                label: String::new(),
                arg_bounds: Vec::new(),
            },
            fact: crate::TraceSpecClauseFact::Deny {
                pattern: row,
                label: String::new(),
            },
            span,
        }];
        for alternatives in [
            vec![],
            vec![vec![]],
            vec![valid.clone(), vec![]],
            vec![vec![], valid],
        ] {
            let diagnostics = validate_alternatives(
                TraceSpecEnv {
                    types: &types,
                    registry: &registry,
                },
                &alternatives,
                &crate::ActionTraceDomain::Empty,
                &Default::default(),
                &Default::default(),
                span,
                (None, None),
            );
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
                "{diagnostics:?}"
            );
        }
        let diagnostics = validate_clauses(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &[],
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (None, None),
        );
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)));
        let diagnostics = validate_conformances(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &Default::default(),
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (None, None, None),
        );
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)));
    }

    #[test]
    fn missing_temporal_analysis_is_not_recomputed_or_hidden_by_an_or_branch() {
        use crate::trace_spec::{
            TraceSpecClause, TraceSpecPattern,
            analysis::{ClauseOutcome, TemporalClauseOutcome, TemporalStaticDisposition},
        };
        let span = etas_core::Span::new(
            SourceId(4),
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(4)),
        );
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name| crate::Effect::Action(registry.action_by_name(name).unwrap());
        let row = |name| crate::EffectRow::closed(crate::EffectSet::one(action(name)));
        let guard_row = row("Console.stdin_read_line");
        let target_row = row("Console.stdout_write");
        let guard = TraceSpecPattern {
            row: guard_row.clone(),
            label: String::new(),
            arg_bounds: vec![],
        };
        let target = TraceSpecPattern {
            row: target_row.clone(),
            label: String::new(),
            arg_bounds: vec![],
        };
        let allow = TraceSpecClause::Allow {
            source_path: None,
            source_clause: None,
            pattern: guard.clone(),
            fact: crate::TraceSpecClauseFact::Allow {
                pattern: guard_row.clone(),
                label: String::new(),
            },
            span,
        };
        let before = TraceSpecClause::RequireBefore {
            source_path: None,
            source_clause: None,
            guard: guard.clone(),
            target: target.clone(),
            fact: crate::TraceSpecClauseFact::RequireBefore {
                guard: guard_row,
                guard_label: String::new(),
                target: target_row,
                target_label: String::new(),
            },
            span,
        };
        let alternatives = vec![vec![allow], vec![before.clone()]];
        let malformed = vec![vec![None], vec![]];
        let diagnostics = validate_alternatives(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &alternatives,
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (Some(&malformed), None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
            "{diagnostics:?}"
        );
        let missing = vec![None];
        let diagnostics = validate_clauses(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            std::slice::from_ref(&before),
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (Some(&missing), None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
            "{diagnostics:?}"
        );
        let violation = vec![Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
            source_clause: None,
            guard,
            target,
            disposition: TemporalStaticDisposition::Violated,
        }))];
        let diagnostics = validate_clauses(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &[before],
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (Some(&violation), None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecRequirementNotDominating)),
            "the validation stage must consume the supplied analysis: {diagnostics:?}"
        );
    }

    #[test]
    fn temporal_outcome_from_another_source_clause_is_rejected() {
        use crate::trace_spec::{
            TraceSpecClause, TraceSpecPattern,
            analysis::{ClauseOutcome, TemporalClauseOutcome, TemporalStaticDisposition},
        };
        use etas_code::{CodeFingerprint, residual::identity::ClauseId};
        let span = etas_core::Span::new(
            SourceId(4),
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(4)),
        );
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let row = crate::EffectRow::closed(crate::EffectSet::one(crate::Effect::Action(
            registry.action_by_name("Console.stdout_write").unwrap(),
        )));
        let pattern = TraceSpecPattern {
            row: row.clone(),
            label: String::new(),
            arg_bounds: vec![],
        };
        let clause_id =
            |byte| ClauseId::from_canonical_fingerprint(CodeFingerprint::from_bytes([byte; 32]));
        let clauses = vec![vec![TraceSpecClause::RequireBefore {
            source_path: Some(vec![0]),
            source_clause: Some(clause_id(1)),
            guard: pattern.clone(),
            target: pattern.clone(),
            fact: crate::TraceSpecClauseFact::RequireBefore {
                guard: row.clone(),
                guard_label: String::new(),
                target: row,
                target_label: String::new(),
            },
            span,
        }]];
        let stale = vec![vec![Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
            source_clause: Some(clause_id(2)),
            guard: pattern.clone(),
            target: pattern.clone(),
            disposition: TemporalStaticDisposition::Proven,
        }))]];
        let diagnostics = validate_alternatives(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &clauses,
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (Some(&stale), None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
            "{diagnostics:?}"
        );
        let missing = vec![vec![Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
            source_clause: None,
            guard: pattern.clone(),
            target: pattern.clone(),
            disposition: TemporalStaticDisposition::Proven,
        }))]];
        let matching = vec![vec![Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
            source_clause: Some(clause_id(1)),
            guard: pattern.clone(),
            target: pattern.clone(),
            disposition: TemporalStaticDisposition::Proven,
        }))]];
        let mut other_pattern = pattern.clone();
        other_pattern.row = crate::EffectRow::closed(crate::EffectSet::one(crate::Effect::Action(
            registry.action_by_name("Console.stdin_read_line").unwrap(),
        )));
        let wrong_instance = vec![vec![Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
            source_clause: Some(clause_id(1)),
            guard: other_pattern,
            target: pattern,
            disposition: TemporalStaticDisposition::Proven,
        }))]];
        for (outcomes, should_pass) in [
            (&missing, false),
            (&matching, true),
            (&wrong_instance, false),
        ] {
            let diagnostics = validate_alternatives(
                TraceSpecEnv {
                    types: &types,
                    registry: &registry,
                },
                &clauses,
                &crate::ActionTraceDomain::Empty,
                &Default::default(),
                &Default::default(),
                span,
                (Some(outcomes), None),
            );
            assert_eq!(diagnostics.is_empty(), should_pass, "{diagnostics:?}");
        }
        let open = crate::EffectRow {
            effects: Default::default(),
            open: Some(crate::effect_var_id_from_name("E")),
        };
        let diagnostics = validate_alternatives(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &clauses,
            &crate::ActionTraceDomain::Empty,
            &open,
            &open,
            span,
            (Some(&matching), None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains("prior-action analysis")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn deny_analysis_cannot_be_reused_with_other_requested_actions() {
        use crate::trace_spec::{
            TraceSpecClause, TraceSpecPattern,
            analysis::{ClauseOutcome, DenialClauseOutcome, DenialStaticDisposition},
        };
        let span = etas_core::Span::new(
            SourceId(4),
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(4)),
        );
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let row = crate::EffectRow::closed(crate::EffectSet::one(crate::Effect::Action(
            registry.action_by_name("Console.stdout_write").unwrap(),
        )));
        let pattern = TraceSpecPattern {
            row: row.clone(),
            label: String::new(),
            arg_bounds: vec![],
        };
        let clauses = vec![vec![TraceSpecClause::Deny {
            source_path: None,
            source_clause: None,
            pattern: pattern.clone(),
            fact: crate::TraceSpecClauseFact::Deny {
                pattern: row.clone(),
                label: String::new(),
            },
            span,
        }]];
        let stale = vec![vec![Some(ClauseOutcome::Denial(DenialClauseOutcome {
            source_clause: None,
            pattern,
            requested: std::sync::Arc::new(crate::EffectRow::empty()),
            unresolved_calls: false,
            disposition: DenialStaticDisposition::Proven,
        }))]];
        let diagnostics = validate_alternatives(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &clauses,
            &crate::ActionTraceDomain::Empty,
            &row,
            &row,
            span,
            (Some(&stale), None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn permission_analysis_cannot_be_reused_for_another_alternative() {
        use crate::trace_spec::{
            TraceSpecClause, TraceSpecPattern, analysis::analyze_permissions,
            model::normalize_alternative,
        };
        let span = etas_core::Span::new(
            SourceId(4),
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(4)),
        );
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name| crate::Effect::Action(registry.action_by_name(name).unwrap());
        let clause = |effect| {
            let row = crate::EffectRow::closed(crate::EffectSet::one(effect));
            TraceSpecClause::Allow {
                source_path: None,
                source_clause: None,
                pattern: TraceSpecPattern {
                    row: row.clone(),
                    label: String::new(),
                    arg_bounds: vec![],
                },
                fact: crate::TraceSpecClauseFact::Allow {
                    pattern: row,
                    label: String::new(),
                },
                span,
            }
        };
        let first = vec![clause(action("Console.stdout_write"))];
        let second = vec![clause(action("Console.stdin_read_line"))];
        let row = crate::EffectRow::closed(crate::EffectSet::one(action("Console.stdout_write")));
        let normalized = normalize_alternative(&first).unwrap();
        let proof = analyze_permissions(
            &first,
            &normalized.permissions,
            std::sync::Arc::new(row.clone()),
            false,
            &registry,
            &types,
        )
        .unwrap();
        let results = vec![Some(proof)];
        let diagnostics = validate_alternatives(
            TraceSpecEnv {
                types: &types,
                registry: &registry,
            },
            &vec![second],
            &crate::ActionTraceDomain::Empty,
            &row,
            &row,
            span,
            (None, Some(&results)),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn external_trace_validation_preserves_order_and_rejects_missing_trace() {
        use crate::trace_spec::{
            TraceSpecPattern,
            monitor::{self, TemporalMonitorResult},
        };
        use crate::{
            ActionEventSource, ExternalActionTraceMetadata as Trace, ExternalEffectMetadata,
            ExternalEffectRowMetadata, ExternalEffectSummaryMetadata,
        };
        let span = etas_core::Span::new(
            SourceId(9),
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(4)),
        );
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name: &str| ExternalEffectMetadata {
            path: vec!["Console".into(), name.into()],
            args: vec![],
        };
        let guard = action("stdin_read_line");
        let target = action("stdout_write");
        let event = |action| Trace::Event {
            action,
            source: ActionEventSource::ExternalMetadata,
        };
        let mut summary = ExternalEffectSummaryMetadata {
            item: vec!["dep".into(), "run".into()],
            requested_actions: ExternalEffectRowMetadata {
                effects: vec![guard.clone(), target.clone()],
                tail: None,
            },
            action_trace: Trace::Seq(vec![event(target.clone()), event(guard.clone())]),
            ..Default::default()
        };
        let pattern = |name| TraceSpecPattern {
            row: crate::EffectRow::closed(crate::EffectSet::one(crate::Effect::Action(
                registry.action_by_name(name).unwrap(),
            ))),
            label: String::new(),
            arg_bounds: vec![],
        };
        let guard_pattern = pattern("Console.stdin_read_line");
        let target_pattern = pattern("Console.stdout_write");
        let mut effects = EffectOutput::default();
        let lowered =
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .expect("checked trace");
        assert_eq!(
            monitor::require_before(
                &lowered.action_trace,
                &guard_pattern,
                &target_pattern,
                &registry,
                &types
            ),
            TemporalMonitorResult::Violation
        );
        summary.action_trace = Trace::Seq(vec![event(guard.clone()), event(target.clone())]);
        let lowered =
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .expect("checked trace");
        assert_eq!(
            monitor::require_before(
                &lowered.action_trace,
                &guard_pattern,
                &target_pattern,
                &registry,
                &types
            ),
            TemporalMonitorResult::NeedsOutcomeEvidence
        );
        summary.action_trace = Trace::UnknownOrder(vec![guard, target]);
        let lowered =
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .expect("checked trace");
        assert_eq!(
            monitor::require_before(
                &lowered.action_trace,
                &guard_pattern,
                &target_pattern,
                &registry,
                &types
            ),
            TemporalMonitorResult::Unknown
        );
        summary.action_trace = Trace::Empty;
        assert!(
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .is_none()
        );
        summary.requested_actions.effects = vec![action("stdin_read_line")];
        summary.action_trace = Trace::Seq(vec![
            event(action("stdin_read_line")),
            event(action("stdout_write")),
        ]);
        assert!(
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .is_none(),
            "a trace-only action must not evade deny checks"
        );
        summary.action_trace = Trace::Empty;
        summary.requested_actions.effects.clear();
        summary.requested_actions.tail = Some("E".into());
        assert!(
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .is_none(),
            "an open row must not become an empty summary"
        );
        summary.requested_actions.tail = None;
        summary.action_trace = Trace::Choice(vec![]);
        assert!(
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .is_none()
        );
        let mut nested = Trace::Empty;
        for _ in 0..=etas_package_metadata::MAX_METADATA_GRAPH_DEPTH {
            nested = Trace::Repeat(Box::new(nested));
        }
        summary.action_trace = nested;
        assert!(
            trace_summary_from_external_metadata(&summary, &types, &registry, &mut effects, span)
                .is_none()
        );
        assert!(effects.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)));
    }

    #[test]
    fn unimplemented_trace_limit_is_not_silently_accepted() {
        let source = SourceFile::new(SourceId(0), None, "limit".to_owned());
        let span = etas_core::Span::new(
            source.id,
            etas_core::TextRange::new(etas_core::TextSize(0), etas_core::TextSize(5)),
        );
        let diagnostics = validate_clauses(
            TraceSpecEnv {
                types: &etas_types::TypeOutput::default(),
                registry: &EffectRegistry::with_standard_effects(),
            },
            &[crate::trace_spec::TraceSpecClause::Limit {
                fact: crate::TraceSpecClauseFact::Limit { count: 1 },
            }],
            &crate::ActionTraceDomain::Empty,
            &Default::default(),
            &Default::default(),
            span,
            (None, None),
        );
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)));

        let registry = EffectRegistry::with_standard_effects();
        let action =
            crate::Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let row = crate::EffectRow::closed(crate::EffectSet::one(action.clone()));
        let diagnostics = validate_clauses(
            TraceSpecEnv {
                types: &etas_types::TypeOutput::default(),
                registry: &registry,
            },
            &[
                crate::trace_spec::TraceSpecClause::Limit {
                    fact: crate::TraceSpecClauseFact::Limit { count: 1 },
                },
                crate::trace_spec::TraceSpecClause::Deny {
                    source_path: None,
                    source_clause: None,
                    pattern: crate::trace_spec::TraceSpecPattern {
                        row: row.clone(),
                        label: String::new(),
                        arg_bounds: Vec::new(),
                    },
                    fact: crate::TraceSpecClauseFact::Deny {
                        pattern: row,
                        label: String::new(),
                    },
                    span,
                },
            ],
            &crate::ActionTraceDomain::Empty,
            &crate::EffectRow::closed(crate::EffectSet::one(action)),
            &Default::default(),
            span,
            (None, None),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
            "an unsupported limit must not hide a definite denial: {diagnostics:#?}"
        );
    }

    #[test]
    fn missing_materialized_item_effect_facts_fail_closed() {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(0),
            None,
            "flow main() -> unit { return; }",
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let item = hir
            .items
            .iter()
            .find_map(|(item, data)| matches!(data, HirItem::Flow(_)).then_some(item))
            .expect("flow item");
        let mut models = TraceSpecModelStore::default();
        models.referenced_by_item.insert(item, vec![vec![vec![]]]);
        let mut summaries = SummaryStore::new();
        summaries.insert(
            EffectUnit::Item(item),
            TraceSpecSummary::for_unit(EffectUnit::Item(item)),
        );
        let mut effects = EffectOutput::default();

        validate_item_policies(
            &hir,
            TraceSpecEnv {
                types: &etas_types::TypeOutput::default(),
                registry: &EffectRegistry::with_standard_effects(),
            },
            &mut effects,
            &models,
            (
                &TraceSpecAnalysisOutput {
                    summaries,
                    source_conformances: Default::default(),
                    clause_outcomes: Default::default(),
                    permission_outcomes: Default::default(),
                    external_outcomes: Default::default(),
                },
                &super::super::TraceStaticDispositions::default(),
                &Default::default(),
            ),
            None,
            None,
        )
        .expect("missing facts should become a diagnostic");

        assert!(effects.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic
                    .message
                    .contains("materialized item effect facts")
        }));
    }

    #[test]
    fn local_trace_and_requested_action_mismatch_fails_closed_in_both_directions() {
        use crate::{
            ActionEvent, ActionEventSource, ActionTraceDomain, Effect, EffectRow, EffectSet,
        };
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(17),
            None,
            "flow main() -> unit { return; }",
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let (item, flow) = hir
            .items
            .iter()
            .find(|(_, item)| matches!(item, HirItem::Flow(_)))
            .unwrap();
        let registry = EffectRegistry::with_standard_effects();
        let action = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let trace = ActionTraceDomain::Event(ActionEvent {
            action: action.clone(),
            span: flow.span(),
            source: ActionEventSource::Perform,
        });
        let row = EffectRow::closed(EffectSet::one(action));
        let mut models = TraceSpecModelStore::default();
        models.referenced_by_item.insert(item, vec![vec![vec![]]]);
        for (action_trace, requested_actions) in [
            (ActionTraceDomain::Empty, row.clone()),
            (trace.clone(), EffectRow::empty()),
        ] {
            let mut summary = TraceSpecSummary::for_unit(EffectUnit::Item(item));
            summary.action_trace = action_trace;
            summary.requested_actions = requested_actions;
            let mut summaries = SummaryStore::new();
            summaries.insert(EffectUnit::Item(item), summary);
            let mut effects = EffectOutput::default();
            effects
                .facts
                .item_effects
                .insert(item, EffectSummary::local());
            validate_item_policies(
                &hir,
                TraceSpecEnv {
                    types: &etas_types::TypeOutput::default(),
                    registry: &registry,
                },
                &mut effects,
                &models,
                (
                    &TraceSpecAnalysisOutput {
                        summaries,
                        source_conformances: Default::default(),
                        clause_outcomes: Default::default(),
                        permission_outcomes: Default::default(),
                        external_outcomes: Default::default(),
                    },
                    &super::super::TraceStaticDispositions::default(),
                    &Default::default(),
                ),
                None,
                None,
            )
            .unwrap();
            assert!(
                effects.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code
                        == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                        && diagnostic
                            .message
                            .contains("trace spec action trace and requested actions")
                        && diagnostic.primary.span.source == SourceId(17)
                }),
                "{:?}",
                effects.diagnostics
            );
            assert!(matches!(
                effects.facts.item_effects[&item].support,
                crate::InterpreterSupport::Rejected(_)
            ));
        }
    }

    #[test]
    fn unresolved_residual_summary_is_rejected_at_its_real_source() {
        use etas_code::{
            CodeFingerprint,
            residual::identity::{ObligationId, ResidualObligationRef},
        };
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(17),
            None,
            "flow main() -> unit { return; }",
        ));
        assert!(parsed.diagnostics.is_empty());
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        let item = hir
            .items
            .iter()
            .find_map(|(item, data)| matches!(data, HirItem::Flow(_)).then_some(item))
            .unwrap();
        let mut summary = EffectSummary::local();
        summary.residual_checks.insert(
            ResidualObligationRef::new(
                ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
                Default::default(),
            )
            .unwrap(),
        );
        let mut effects = EffectOutput::default();
        effects.facts.item_effects.insert(item, summary.clone());
        effects
            .facts
            .unit_effects
            .insert(EffectUnit::Item(item), summary);
        reject_uncovered_residuals(&hir, &mut effects, &Default::default(), None).unwrap();
        assert_eq!(effects.diagnostics.len(), 1);
        assert_eq!(
            effects.diagnostics[0].code,
            DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
        );
        assert_eq!(effects.diagnostics[0].primary.span.source, SourceId(17));
        assert!(matches!(
            effects.facts.item_effects[&item].support,
            crate::InterpreterSupport::Rejected(_)
        ));
    }

    #[test]
    fn missing_residual_anchor_fails_without_fabricating_a_diagnostic() {
        use etas_code::{
            CodeFingerprint,
            residual::identity::{ObligationId, ResidualObligationRef},
        };
        let hir = etas_hir::HirRevision::try_new(etas_hir::HirProgram::default()).unwrap();
        let mut summary = EffectSummary::local();
        summary.residual_checks.insert(
            ResidualObligationRef::new(
                ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
                Default::default(),
            )
            .unwrap(),
        );
        let mut effects = EffectOutput::default();
        effects
            .facts
            .unit_effects
            .insert(EffectUnit::Item(0_u32.into()), summary);
        assert!(matches!(
            reject_uncovered_residuals(&hir, &mut effects, &Default::default(), None),
            Err(crate::EffectPipelineError::MissingDiagnosticAnchor { .. })
        ));
        assert!(effects.diagnostics.is_empty());
    }

    #[test]
    fn expression_and_statement_residuals_use_checked_owners_and_reachability() {
        use etas_code::{
            CodeFingerprint,
            residual::identity::{ObligationId, ResidualObligationRef},
        };
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(23),
            None,
            "flow main() -> unit { let x = 1; return; }",
        ));
        assert!(parsed.diagnostics.is_empty());
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        let item = hir
            .items
            .iter()
            .find_map(|(item, data)| matches!(data, HirItem::Flow(_)).then_some(item))
            .unwrap();
        let expr = *hir
            .index()
            .expr_item
            .iter()
            .find_map(|(expr, owner)| (*owner == item).then_some(expr))
            .unwrap();
        let stmt = *hir
            .index()
            .stmt_item
            .iter()
            .find_map(|(stmt, owner)| (*owner == item).then_some(stmt))
            .unwrap();
        let mut summary = EffectSummary::local();
        summary.residual_checks.insert(
            ResidualObligationRef::new(
                ObligationId::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32])),
                Default::default(),
            )
            .unwrap(),
        );
        let mut effects = EffectOutput::default();
        effects.facts.expr_effects.insert(expr, summary.clone());
        effects.facts.stmt_effects.insert(stmt, summary);
        let mut unreachable = effects.clone();
        reject_uncovered_residuals(
            &hir,
            &mut unreachable,
            &Default::default(),
            Some(&Default::default()),
        )
        .unwrap();
        assert!(unreachable.diagnostics.is_empty());
        reject_uncovered_residuals(
            &hir,
            &mut effects,
            &Default::default(),
            Some(&[item].into_iter().collect()),
        )
        .unwrap();
        assert_eq!(effects.diagnostics.len(), 2);
        assert!(
            effects
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.primary.span.source == SourceId(23))
        );
    }

    #[test]
    fn latent_residual_is_rejected_even_if_unit_summary_is_empty() {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(24),
            None,
            "flow main() -> unit { let x = 1; return; }",
        ));
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        let item = hir
            .items
            .iter()
            .find_map(|(item, data)| matches!(data, HirItem::Flow(_)).then_some(item))
            .unwrap();
        let expr = *hir
            .index()
            .expr_item
            .iter()
            .find_map(|(expr, owner)| (*owner == item).then_some(expr))
            .unwrap();
        let mut effects = EffectOutput::default();
        effects.facts.latent_effects.insert(
            expr,
            crate::LatentEffectFact {
                value: expr,
                body: crate::LatentEffectBodyRef::Expr(expr),
                flow_type: etas_types::TypeId(0),
                inferred: Default::default(),
                declared_bound: None,
                summary: residual_summary(),
                realized_at: vec![],
            },
        );
        reject_uncovered_residuals(
            &hir,
            &mut effects,
            &Default::default(),
            Some(&[item].into_iter().collect()),
        )
        .unwrap();
        assert_eq!(effects.diagnostics.len(), 1);
        assert_eq!(effects.diagnostics[0].primary.span.source, SourceId(24));
    }

    #[test]
    fn forged_anonymous_unit_owner_is_rejected_before_reachability_filtering() {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(25),
            None,
            "flow main() -> unit { let x = 1; return; } flow other() -> unit { return; }",
        ));
        let hir = etas_hir::HirRevision::try_new(lower_program(&parsed.value)).unwrap();
        let items = hir
            .items
            .iter()
            .filter_map(|(item, data)| matches!(data, HirItem::Flow(_)).then_some(item))
            .collect::<Vec<_>>();
        let expr = *hir
            .index()
            .expr_item
            .iter()
            .find_map(|(expr, owner)| (*owner == items[0]).then_some(expr))
            .unwrap();
        let unit = EffectUnit::AnonymousFlow {
            owner: items[1],
            value: expr,
            body: crate::infer::unit::EffectAnonymousFlowBody::Expr(expr),
        };
        let mut effects = EffectOutput::default();
        effects.facts.unit_effects.insert(unit, residual_summary());
        assert!(matches!(
            reject_uncovered_residuals(
                &hir,
                &mut effects,
                &Default::default(),
                Some(&[items[1]].into_iter().collect())
            ),
            Err(crate::EffectPipelineError::MissingDiagnosticAnchor { .. })
        ));
        assert!(effects.diagnostics.is_empty());
    }

    #[test]
    fn unanchored_external_callable_residual_returns_structural_error() {
        let hir = etas_hir::HirRevision::try_new(etas_hir::HirProgram::default()).unwrap();
        let source = crate::MetadataCallableSource {
            graph: etas_code::CodeFingerprint::from_bytes([3; 32]),
            value: 0,
            environment: None,
            analysis: crate::CallableAnalysisKind::CheckedRevision,
        };
        let mut effects = EffectOutput::default();
        effects.facts.metadata_callables.insert(
            source,
            crate::facts::MetadataCallableAnalysis::Solved {
                summary: std::sync::Arc::new(residual_summary()),
                type_bindings: vec![],
                effect_row_bindings: vec![],
            },
        );
        assert!(matches!(
            reject_uncovered_residuals(&hir, &mut effects, &Default::default(), None),
            Err(crate::EffectPipelineError::MissingDiagnosticAnchor { .. })
        ));
        assert!(effects.diagnostics.is_empty());
    }
}
