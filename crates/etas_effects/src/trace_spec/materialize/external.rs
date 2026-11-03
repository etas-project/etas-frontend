use etas_core::Span;
use etas_hir::{HirItem, HirSpecKind, SymbolDef, SymbolId};

use crate::trace_spec::model::{
    TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecModelStore, TraceSpecPattern,
};
use crate::{
    ActionInstanceRef, Effect, EffectRow, EffectSet, ExternalTraceSpecClauseKind,
    ExternalTraceSpecClauseMetadata, ExternalTraceSpecEffectMetadata,
    ExternalTraceSpecEffectRowMetadata, ExternalTraceSpecSummaryMetadata, TraceSpecClauseFact,
};

use super::{TraceSpecMaterializer, external_row_label};

impl TraceSpecMaterializer<'_> {
    pub(super) fn trace_spec_clauses_for_symbol(
        &mut self,
        symbol: SymbolId,
        span: Span,
        store: &TraceSpecModelStore,
    ) -> Option<(String, TraceSpecClauseAlternatives)> {
        let Some(declaration) = self.hir.symbols.get(symbol) else {
            self.diagnostic(span, "trace spec conformance is missing its checked symbol");
            return None;
        };
        if let Some(spec_item) = declaration.defining_item.filter(|item| matches!(self.hir.items.get(*item), Some(HirItem::Spec(spec)) if matches!(spec.kind, HirSpecKind::TraceSpec))) {
            let Some(clauses) = store.clauses_by_item.get(&spec_item).cloned() else {
                self.diagnostic(
                    span,
                    "trace spec conformance requires materialized trace spec clauses",
                );
                return None;
            };
            let Some(name) = store
                .trace_spec_names
                .get(&spec_item)
                .cloned() else {
                    self.diagnostic(span, "trace spec declaration identity is missing");
                    return None;
                };
            return Some((name, clauses));
        }

        let SymbolDef::ImportAlias { .. } = &declaration.def else {
            self.diagnostic(
                span,
                "trace spec conformance must resolve to a source or external trace spec",
            );
            return None;
        };
        let Ok(summary) = checked_external_summary(self.types, symbol, self.external_trace_specs)
        else {
            self.diagnostic(
                span,
                "external trace spec requires unique checked package metadata",
            );
            return None;
        };
        let summary = summary.metadata.clone();
        let name = summary.trace_spec.join(".");
        let clauses = self.clauses_from_external_summary(&summary, span)?;
        Some((name, clauses))
    }

    pub(super) fn clauses_from_external_summary(
        &mut self,
        summary: &ExternalTraceSpecSummaryMetadata,
        span: Span,
    ) -> Option<TraceSpecClauseAlternatives> {
        if summary.alternatives.is_empty() || summary.alternatives.iter().any(Vec::is_empty) {
            self.diagnostic(
                span,
                format!(
                    "external trace spec `{}` has no materialized trace clauses",
                    summary.trace_spec.join(".")
                ),
            );
            return None;
        }
        summary
            .alternatives
            .iter()
            .map(|clauses| {
                clauses
                    .iter()
                    .map(|clause| self.clause_from_external_metadata(clause, span))
                    .collect::<Option<Vec<_>>>()
            })
            .collect()
    }

    fn clause_from_external_metadata(
        &mut self,
        clause: &ExternalTraceSpecClauseMetadata,
        span: Span,
    ) -> Option<TraceSpecClause> {
        match clause.kind {
            ExternalTraceSpecClauseKind::Allow => {
                let pattern = self.external_pattern(clause.pattern.as_ref(), span, "allow")?;
                let fact = TraceSpecClauseFact::Allow {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Some(TraceSpecClause::Allow {
                    source_path: None,
                    source_clause: None,
                    pattern,
                    fact,
                    span,
                })
            }
            ExternalTraceSpecClauseKind::Deny => {
                let pattern = self.external_pattern(clause.pattern.as_ref(), span, "deny")?;
                let fact = TraceSpecClauseFact::Deny {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Some(TraceSpecClause::Deny {
                    source_path: None,
                    source_clause: None,
                    pattern,
                    fact,
                    span,
                })
            }
            ExternalTraceSpecClauseKind::RequireBefore => {
                let guard =
                    self.external_pattern(clause.guard.as_ref(), span, "require-before guard")?;
                let target =
                    self.external_pattern(clause.target.as_ref(), span, "require-before target")?;
                let fact = TraceSpecClauseFact::RequireBefore {
                    guard: guard.row.clone(),
                    guard_label: guard.label.clone(),
                    target: target.row.clone(),
                    target_label: target.label.clone(),
                };
                Some(TraceSpecClause::RequireBefore {
                    source_path: None,
                    source_clause: None,
                    guard,
                    target,
                    fact,
                    span,
                })
            }
            ExternalTraceSpecClauseKind::RequireAfter => {
                let target =
                    self.external_pattern(clause.target.as_ref(), span, "require-after target")?;
                let obligation = self.external_pattern(
                    clause.obligation.as_ref(),
                    span,
                    "require-after obligation",
                )?;
                let fact = TraceSpecClauseFact::RequireAfter {
                    target: target.row.clone(),
                    target_label: target.label.clone(),
                    obligation: obligation.row.clone(),
                    obligation_label: obligation.label.clone(),
                };
                Some(TraceSpecClause::RequireAfter {
                    source_path: None,
                    source_clause: None,
                    target,
                    obligation,
                    fact,
                    span,
                })
            }
        }
    }

    fn external_pattern(
        &mut self,
        row: Option<&ExternalTraceSpecEffectRowMetadata>,
        span: Span,
        role: &str,
    ) -> Option<TraceSpecPattern> {
        let Some(row) = row else {
            self.diagnostic(
                span,
                format!("external trace spec {role} pattern is missing"),
            );
            return None;
        };
        let row = self.external_effect_row(row, span)?;
        Some(TraceSpecPattern {
            label: external_row_label(&row, self.registry),
            row,
            arg_bounds: Vec::new(),
        })
    }

    fn external_effect_row(
        &mut self,
        row: &ExternalTraceSpecEffectRowMetadata,
        span: Span,
    ) -> Option<EffectRow> {
        let mut effects = EffectSet::new();
        for effect in &row.effects {
            effects.insert(self.external_effect(effect, span)?);
        }
        Some(EffectRow::closed(effects))
    }

    fn external_effect(
        &mut self,
        effect: &ExternalTraceSpecEffectMetadata,
        span: Span,
    ) -> Option<Effect> {
        let qualified = effect.path.join(".");
        if let Some(action) = self.registry.action_by_name(&qualified) {
            let Some(signature) = self.registry.action_signature(&action) else {
                self.diagnostic(
                    span,
                    format!(
                        "external trace spec action `{qualified}` is missing registry signature"
                    ),
                );
                return None;
            };
            if effect.args.len() != signature.effect_args.len() {
                self.diagnostic(
                    span,
                    format!(
                        "external trace spec action `{qualified}` expects {} static selector argument(s), got {}",
                        signature.effect_args.len(),
                        effect.args.len()
                    ),
                );
                return None;
            }
            return Some(if effect.args.is_empty() {
                Effect::Action(action)
            } else {
                Effect::AppliedAction(ActionInstanceRef {
                    action,
                    args: effect.args.clone(),
                })
            });
        }
        if let Some(tag) = self.registry.tag_by_name(&qualified) {
            if !effect.args.is_empty() {
                self.diagnostic(
                    span,
                    format!(
                        "external trace spec effect tag `{qualified}` arguments require checked type facts"
                    ),
                );
                return None;
            }
            return Some(Effect::Tag(tag));
        }
        self.diagnostic(
            span,
            format!("external trace spec effect `{qualified}` is not in the effect registry"),
        );
        None
    }
}

pub(in crate::trace_spec) fn checked_external_summary<'a>(
    types: &etas_types::TypeOutput,
    symbol: SymbolId,
    summaries: &'a [crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
) -> Result<&'a crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>, &'static str> {
    let identity = types
        .facts
        .external_specs
        .aliases
        .get(&symbol)
        .ok_or("external trace spec has no checked package identity")?;
    let signature = types
        .facts
        .external_specs
        .signatures
        .get(identity)
        .ok_or("external trace spec has no checked signature")?;
    if signature.kind != etas_types::SpecKind::TraceSpec {
        return Err("external spec is not a trace spec");
    }
    let mut matching = summaries.iter().filter(|summary| {
        summary.package == identity.package && summary.metadata.trace_spec == identity.path
    });
    let result = matching
        .next()
        .ok_or("external trace spec metadata is missing")?;
    if matching.next().is_some() {
        return Err("external trace spec metadata is ambiguous");
    }
    Ok(result)
}
