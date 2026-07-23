use etas_core::Span;
use etas_hir::{HirItem, HirItemId, HirSpecKind, ResolveResult, SymbolDef};

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
    pub(super) fn trace_spec_clauses_for_ref(
        &mut self,
        spec_ref: &etas_hir::HirSpecRef,
        store: &TraceSpecModelStore,
    ) -> Option<(String, TraceSpecClauseAlternatives)> {
        if let Some(spec_item) = self.trace_spec_item_for_ref(spec_ref) {
            let Some(clauses) = store.clauses_by_item.get(&spec_item).cloned() else {
                self.diagnostic(
                    spec_ref.span,
                    "trace spec conformance requires materialized trace spec clauses",
                );
                return None;
            };
            let name = store
                .trace_spec_names
                .get(&spec_item)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned());
            return Some((name, clauses));
        }

        let Some(path) = self.external_trace_spec_path(spec_ref) else {
            self.diagnostic(
                spec_ref.span,
                "trace spec conformance must resolve to a source or external trace spec",
            );
            return None;
        };
        let Some(summary) = self
            .external_trace_specs
            .iter()
            .find(|summary| summary.trace_spec == path)
        else {
            self.diagnostic(
                spec_ref.span,
                format!(
                    "external trace spec `{}` requires package metadata trace clauses",
                    path.join(".")
                ),
            );
            return None;
        };
        let summary = summary.metadata.clone();
        let clauses = self.clauses_from_external_summary(&summary, spec_ref.span)?;
        Some((path.join("."), clauses))
    }

    fn external_trace_spec_path(&self, spec_ref: &etas_hir::HirSpecRef) -> Option<Vec<String>> {
        let ResolveResult::Resolved(symbol) = spec_ref.spec_path.resolution else {
            return None;
        };
        let symbol = self.hir.symbols.get(symbol)?;
        let SymbolDef::ImportAlias { path, origin: _ } = &symbol.def else {
            return None;
        };
        Some(path.clone())
    }

    fn trace_spec_item_for_ref(&mut self, spec_ref: &etas_hir::HirSpecRef) -> Option<HirItemId> {
        let ResolveResult::Resolved(symbol) = spec_ref.spec_path.resolution else {
            return None;
        };
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            self.diagnostic(
                spec_ref.span,
                "trace spec conformance is missing symbol facts",
            );
            return None;
        };
        let item = symbol.defining_item?;
        match self.hir.items.get(item) {
            Some(HirItem::Spec(spec)) if matches!(spec.kind, HirSpecKind::TraceSpec) => Some(item),
            _ => None,
        }
    }

    pub(super) fn clauses_from_external_summary(
        &mut self,
        summary: &ExternalTraceSpecSummaryMetadata,
        span: Span,
    ) -> Option<TraceSpecClauseAlternatives> {
        if summary.clauses.is_empty() {
            self.diagnostic(
                span,
                format!(
                    "external trace spec `{}` has no materialized trace clauses",
                    summary.trace_spec.join(".")
                ),
            );
            return None;
        }
        let clauses = summary
            .clauses
            .iter()
            .map(|clause| self.clause_from_external_metadata(clause, span))
            .collect::<Option<Vec<_>>>()?;
        Some(vec![clauses])
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
