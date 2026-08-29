use std::collections::BTreeSet;

use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::HirItemId;
use etas_hir_analysis::interprocedural::SummaryStore;

use crate::{
    ActionEvent, ActionEventSource, ActionInstanceRef, ActionTraceDomain, Effect, EffectCoverage,
    EffectOutput, EffectPipelineError, EffectRegistry, EffectRow, EffectSet, EffectUnit,
    ExternalEffectMetadata, ExternalEffectSummaryMetadata, FrontendRejectionReason,
    InterpreterSupport,
};

use super::analysis::TraceSpecAnalysisOutput;
use super::domain::TraceSpecSummary;
use super::model::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecModelStore};
use super::monitor::{self, TemporalMonitorResult};
use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic, resolve_anchor};

pub struct TraceSpecValidationInput<'a> {
    pub hir: &'a etas_hir::HirProgram,
    pub types: &'a etas_types::TypeOutput,
    pub registry: &'a EffectRegistry,
    pub models: &'a TraceSpecModelStore,
    pub analysis: &'a TraceSpecAnalysisOutput,
    pub external_summaries: &'a [crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
    pub reachable_items: Option<&'a BTreeSet<HirItemId>>,
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
        external_summaries,
        reachable_items,
    } = input;
    validate_item_policies(
        hir,
        types,
        registry,
        effects,
        models,
        &analysis.summaries,
        reachable_items,
    )?;
    validate_external_item_policies(hir, types, registry, effects, models, external_summaries)
}

fn validate_item_policies(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    summaries: &SummaryStore<EffectUnit, TraceSpecSummary>,
    reachable_items: Option<&BTreeSet<HirItemId>>,
) -> Result<(), EffectPipelineError> {
    for (item, alternatives) in &models.referenced_by_item {
        if reachable_items.is_some_and(|items| !items.contains(item)) {
            continue;
        }
        let Some(trace_spec_summary) = summaries.get(EffectUnit::Item(*item)) else {
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
        let action_trace = trace_spec_summary.action_trace.clone();
        let requested_actions = trace_spec_summary.requested_actions.clone();
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
        .subtract_handled(&requested_actions, &item_effect_summary.default_actions);
        let fallback_span = resolve_anchor(hir, &DiagnosticAnchor::Unit(EffectUnit::Item(*item)))
            .ok_or_else(|| EffectPipelineError::MissingDiagnosticAnchor {
            artifact: format!("trace spec item {item:?}"),
        })?;
        for diagnostic in validate_alternatives(
            types,
            registry,
            alternatives,
            &action_trace,
            requested_actions.effects.iter().cloned().collect(),
            allow_requested_actions.effects.iter().cloned().collect(),
            fallback_span,
        ) {
            effects.diagnostics.push(diagnostic);
            reject_item_for_trace_spec(effects, *item);
        }
        if let Some(summary) = effects.facts.item_effects.get_mut(item)
            && let Some(requirements) = effects.facts.requirements.items.get(item)
        {
            summary.trace_spec_obligations.union_assign(requirements);
        }
        if !matches!(
            hir.items.get(*item),
            Some(etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_))
        ) {
            reject_item_for_trace_spec(effects, *item);
        }
    }
    Ok(())
}

fn validate_external_item_policies(
    _hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    external_summaries: &[crate::AnchoredExternalMetadata<ExternalEffectSummaryMetadata>],
) -> Result<(), EffectPipelineError> {
    for (item, alternatives) in &models.external_referenced_by_item {
        let Some(summary) = external_summaries
            .iter()
            .find(|summary| external_summary_matches_item(summary, item))
        else {
            return Err(EffectPipelineError::InvalidExternalMetadata {
                package: item
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "<external>".to_owned()),
                reason: format!(
                    "trace spec conformance for `{}` has no anchored solved effect summary",
                    item.join(".")
                ),
            });
        };
        let external_span = summary.span;
        let Some(trace) =
            trace_summary_from_external_metadata(summary, types, registry, effects, external_span)
        else {
            continue;
        };
        for diagnostic in validate_alternatives(
            types,
            registry,
            alternatives,
            &trace.action_trace,
            trace.requested_actions.effects.iter().cloned().collect(),
            trace
                .allow_requested_actions
                .effects
                .iter()
                .cloned()
                .collect(),
            external_span,
        ) {
            effects.diagnostics.push(diagnostic);
        }
    }
    Ok(())
}

struct ExternalTraceSummary {
    action_trace: ActionTraceDomain,
    requested_actions: EffectRow,
    allow_requested_actions: EffectRow,
}

fn trace_summary_from_external_metadata(
    summary: &ExternalEffectSummaryMetadata,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    external_span: Span,
) -> Option<ExternalTraceSummary> {
    let mut action_trace = ActionTraceDomain::Empty;
    let mut requested_actions = Vec::new();
    let mut handled_actions = Vec::new();
    let mut handled_effects = BTreeSet::new();
    for handled in &summary.handled_requested_actions.effects {
        let Some(effect) = external_action_effect_from_metadata(registry, handled) else {
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
        let Some(effect) = external_action_effect_from_metadata(registry, action) else {
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
        action_trace.seq_assign(ActionTraceDomain::Event(ActionEvent {
            action: effect.clone(),
            span: external_span,
            source: ActionEventSource::ExternalMetadata,
        }));
        requested_actions.push(effect.clone());
        if handled_effects.contains(&effect) {
            handled_actions.push(effect);
        }
    }
    let requested_actions = EffectRow::closed(EffectSet::from_iter(requested_actions));
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

fn external_action_effect_from_metadata(
    registry: &EffectRegistry,
    metadata: &ExternalEffectMetadata,
) -> Option<Effect> {
    let action = registry.action_by_name(&metadata.path.join("."))?;
    if metadata.args.is_empty() {
        let Some(signature) = registry.action_signature(&action) else {
            return Some(Effect::Action(action));
        };
        if signature.effect_args.is_empty() {
            return Some(Effect::Action(action));
        }
        let args = signature
            .effect_args
            .iter()
            .enumerate()
            .map(|(index, _)| {
                signature
                    .selector_defaults
                    .get(index)
                    .and_then(Option::as_ref)
                    .cloned()
                    .unwrap_or(etas_types::EffectArgRef::Wildcard)
            })
            .collect();
        return Some(Effect::AppliedAction(ActionInstanceRef { action, args }));
    }
    Some(Effect::AppliedAction(ActionInstanceRef {
        action,
        args: metadata.args.clone(),
    }))
}

fn external_summary_matches_item(summary: &ExternalEffectSummaryMetadata, item: &[String]) -> bool {
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

fn validate_alternatives(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    alternatives: &TraceSpecClauseAlternatives,
    trace: &ActionTraceDomain,
    requested_actions: BTreeSet<Effect>,
    allow_requested_actions: BTreeSet<Effect>,
    fallback_span: Span,
) -> Vec<Diagnostic> {
    if alternatives.is_empty() {
        return Vec::new();
    }
    let mut first_failure = Vec::new();
    for clauses in alternatives {
        let diagnostics = validate_clauses(
            types,
            registry,
            clauses,
            trace,
            requested_actions.clone(),
            allow_requested_actions.clone(),
            fallback_span,
        );
        if diagnostics.is_empty() {
            return Vec::new();
        }
        if first_failure.is_empty() {
            first_failure = diagnostics;
        }
    }
    first_failure
}

fn validate_clauses(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    clauses: &[TraceSpecClause],
    trace: &ActionTraceDomain,
    requested_actions: BTreeSet<Effect>,
    allow_requested_actions: BTreeSet<Effect>,
    fallback_span: Span,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let allow = clauses
        .iter()
        .filter_map(|clause| match clause {
            TraceSpecClause::Allow { pattern, span, .. } => Some((pattern, *span)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !allow.is_empty() {
        for action in &allow_requested_actions {
            if !allow
                .iter()
                .any(|(pattern, _)| pattern.matches_effect(action, registry, types))
            {
                let span = action_span(trace, action)
                    .or_else(|| allow.first().map(|(_, span)| *span))
                    .unwrap_or(fallback_span);
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::TraceSpecAllowViolation,
                    span,
                    "trace spec allow clauses do not cover a requested action",
                ));
            }
        }
    }
    for clause in clauses {
        match clause {
            TraceSpecClause::Deny { pattern, span, .. } => {
                if requested_actions
                    .iter()
                    .any(|action| pattern.matches_effect(action, registry, types))
                {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::TraceSpecDenied,
                        *span,
                        "trace spec deny clause rejects a requested action",
                    ));
                }
            }
            TraceSpecClause::RequireBefore {
                guard,
                target,
                span,
                ..
            } => {
                let result = monitor::require_before(trace, guard, target, registry, types);
                if matches!(result, TemporalMonitorResult::Violation) {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::TraceSpecRequirementNotDominating,
                        *span,
                        "trace spec require-before obligation is not satisfied on every action path",
                    ));
                } else if matches!(result, TemporalMonitorResult::Unknown) {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        *span,
                        "trace spec require-before analysis needs ordered action trace facts",
                    ));
                }
            }
            TraceSpecClause::RequireAfter {
                target,
                obligation,
                span,
                ..
            } => {
                let result = monitor::require_after(trace, target, obligation, registry, types);
                if matches!(result, TemporalMonitorResult::Violation) {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::TraceSpecAfterRequirementMissing,
                        *span,
                        "trace spec require-after obligation is not satisfied on every action path",
                    ));
                } else if matches!(result, TemporalMonitorResult::Unknown) {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        *span,
                        "trace spec require-after analysis needs ordered action trace facts",
                    ));
                }
            }
            TraceSpecClause::Allow { .. } | TraceSpecClause::Limit { .. } => {}
        }
    }
    diagnostics
}

fn action_span(trace: &ActionTraceDomain, action: &Effect) -> Option<Span> {
    match trace {
        ActionTraceDomain::Event(event) if &event.action == action => Some(event.span),
        ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
            parts.iter().find_map(|part| action_span(part, action))
        }
        ActionTraceDomain::Repeat(inner) => action_span(inner, action),
        ActionTraceDomain::Empty
        | ActionTraceDomain::Event(_)
        | ActionTraceDomain::ParameterCall { .. }
        | ActionTraceDomain::UnknownOrder(_) => None,
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

    use super::{TraceSpecModelStore, TraceSpecSummary, validate_item_policies};
    use crate::{EffectOutput, EffectRegistry, EffectUnit};

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
        models.referenced_by_item.insert(item, vec![vec![]]);
        let mut summaries = SummaryStore::new();
        summaries.insert(
            EffectUnit::Item(item),
            TraceSpecSummary::for_unit(EffectUnit::Item(item)),
        );
        let mut effects = EffectOutput::default();

        validate_item_policies(
            &hir,
            &etas_types::TypeOutput::default(),
            &EffectRegistry::with_standard_effects(),
            &mut effects,
            &models,
            &summaries,
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
}
