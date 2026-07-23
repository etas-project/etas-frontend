use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirDeclarationConformanceTarget, HirEffectRef, HirItem, HirItemId, HirProgram, HirSpecExpr,
    HirSpecKind, ResolveResult, SymbolId,
};

use crate::{
    Effect, EffectOutput, EffectPipelineError, EffectRegistry, EffectRow,
    ExternalTraceSpecSummaryMetadata, RequirementFact, TraceSpecClauseFact,
    trace_spec::TraceSpecModelStore,
    trace_spec::model::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecPattern},
};

mod external;
mod selector;

pub fn materialize_trace_spec_models(
    hir: &HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    external_trace_specs: &[crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    effects: &mut EffectOutput,
) -> Result<TraceSpecModelStore, EffectPipelineError> {
    TraceSpecMaterializer {
        hir,
        types,
        registry,
        external_trace_specs,
        effects,
    }
    .materialize()
}

pub(super) struct TraceSpecMaterializer<'a> {
    pub(super) hir: &'a HirProgram,
    pub(super) types: &'a etas_types::TypeOutput,
    pub(super) registry: &'a EffectRegistry,
    pub(super) external_trace_specs:
        &'a [crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    pub(super) effects: &'a mut EffectOutput,
}

impl TraceSpecMaterializer<'_> {
    fn materialize(&mut self) -> Result<TraceSpecModelStore, EffectPipelineError> {
        let mut store = TraceSpecModelStore::default();
        for (item, hir_item) in self.hir.items.iter() {
            let HirItem::Spec(spec) = hir_item else {
                continue;
            };
            if !matches!(spec.kind, HirSpecKind::TraceSpec) {
                continue;
            }
            let name = self.symbol_name(spec.symbol);
            store.trace_spec_names.insert(item, name.clone());
            let Some(trace) = &spec.trace else {
                self.diagnostic(
                    spec.span,
                    format!("trace spec `{name}` requires a trace expression"),
                );
                continue;
            };
            let Some(alternatives) = self.alternatives_from_expr(trace)? else {
                continue;
            };
            let facts = facts_from_alternatives(&alternatives);
            self.effects
                .facts
                .trace_specs
                .symbols
                .entry(spec.symbol)
                .or_default()
                .extend(facts.iter().cloned());
            self.effects
                .facts
                .trace_specs
                .items
                .entry(item)
                .or_default()
                .extend(facts);
            store.clauses_by_item.insert(item, alternatives);
        }

        for (item, hir_item) in self.hir.items.iter() {
            let Some((symbol, conformances)) = callable_symbol_and_conformances(hir_item) else {
                continue;
            };
            for conformance in conformances {
                match &conformance.target {
                    HirDeclarationConformanceTarget::Path(spec_ref) => {
                        if !self.is_checked_trace_spec_path_conformance(item, spec_ref) {
                            continue;
                        }
                        let Some((name, alternatives)) =
                            self.trace_spec_clauses_for_ref(spec_ref, &store)
                        else {
                            continue;
                        };
                        let reference = TraceSpecClauseFact::TraceSpecReference { name };
                        let mut facts = Vec::with_capacity(1);
                        facts.push(reference.clone());
                        facts.extend(facts_from_alternatives(&alternatives));
                        self.effects
                            .facts
                            .trace_specs
                            .items
                            .entry(item)
                            .or_default()
                            .extend(facts.clone());
                        self.effects
                            .facts
                            .trace_specs
                            .symbols
                            .entry(symbol)
                            .or_default()
                            .extend(facts);
                        self.record_trace_spec_requirement(item, symbol, reference);
                        store
                            .referenced_by_item
                            .entry(item)
                            .or_default()
                            .extend(alternatives);
                    }
                    HirDeclarationConformanceTarget::InlineTraceSpec(expr) => {
                        if !self.is_checked_inline_trace_spec_conformance(item, expr.span()) {
                            continue;
                        }
                        let Some(alternatives) = self.alternatives_from_expr(expr)? else {
                            continue;
                        };
                        let facts = facts_from_alternatives(&alternatives);
                        self.effects
                            .facts
                            .trace_specs
                            .items
                            .entry(item)
                            .or_default()
                            .extend(facts.clone());
                        self.effects
                            .facts
                            .trace_specs
                            .symbols
                            .entry(symbol)
                            .or_default()
                            .extend(facts);
                        store
                            .referenced_by_item
                            .entry(item)
                            .or_default()
                            .extend(alternatives);
                    }
                    HirDeclarationConformanceTarget::Error { .. } => {}
                }
            }
        }
        for conformance in &self.types.facts.external_trace_spec_conformances {
            match &conformance.target {
                etas_types::ExternalTraceSpecConformanceTarget::Named { spec, .. } => {
                    let Some(summary) = self
                        .external_trace_specs
                        .iter()
                        .find(|summary| summary.trace_spec == *spec)
                    else {
                        self.diagnostic(
                            conformance.span,
                            format!(
                                "external trace spec `{}` requires package metadata trace clauses",
                                spec.join(".")
                            ),
                        );
                        continue;
                    };
                    let Some(alternatives) =
                        self.clauses_from_external_summary(summary, conformance.span)
                    else {
                        continue;
                    };
                    store
                        .external_referenced_by_item
                        .entry(conformance.item.clone())
                        .or_default()
                        .extend(alternatives);
                }
                etas_types::ExternalTraceSpecConformanceTarget::Inline => {
                    self.diagnostic(
                        conformance.span,
                        "external inline trace spec conformance requires materialized trace spec metadata",
                    );
                }
            }
        }
        Ok(store)
    }

    fn is_checked_trace_spec_path_conformance(
        &mut self,
        item: HirItemId,
        spec_ref: &etas_hir::HirSpecRef,
    ) -> bool {
        let ResolveResult::Resolved(spec_symbol) = spec_ref.spec_path.resolution else {
            return false;
        };
        let Some(signature) = self.types.facts.spec_signatures.get(&spec_symbol) else {
            return false;
        };
        if !matches!(signature.kind, etas_types::SpecKind::TraceSpec) {
            return false;
        }
        let found = self.types.facts.trace_spec_conformances.iter().any(|fact| {
            fact.item == item
                && matches!(
                    &fact.target,
                    etas_types::TraceSpecConformanceTarget::Named {
                        spec_symbol: checked_symbol,
                        ..
                    } if *checked_symbol == spec_symbol
                )
        });
        if !found {
            self.diagnostic(
                spec_ref.span,
                "trace spec conformance requires checked type facts",
            );
        }
        found
    }

    fn is_checked_inline_trace_spec_conformance(&mut self, item: HirItemId, span: Span) -> bool {
        let found = self.types.facts.trace_spec_conformances.iter().any(|fact| {
            fact.item == item
                && matches!(fact.target, etas_types::TraceSpecConformanceTarget::Inline)
        });
        if !found {
            self.diagnostic(
                span,
                "inline trace spec conformance requires checked type facts",
            );
        }
        found
    }

    fn record_trace_spec_requirement(
        &mut self,
        item: HirItemId,
        symbol: SymbolId,
        fact: TraceSpecClauseFact,
    ) {
        let requirement = RequirementFact::TraceSpec(fact);
        self.effects
            .facts
            .requirements
            .items
            .entry(item)
            .or_default()
            .insert(requirement.clone());
        self.effects
            .facts
            .requirements
            .symbols
            .entry(symbol)
            .or_default()
            .insert(requirement.clone());
        if let Some(summary) = self.effects.facts.item_effects.get_mut(&item) {
            summary.trace_spec_obligations.insert(requirement);
        }
    }

    fn alternatives_from_expr(
        &mut self,
        expr: &HirSpecExpr,
    ) -> Result<Option<TraceSpecClauseAlternatives>, EffectPipelineError> {
        match expr {
            HirSpecExpr::Atom(pattern) => {
                let Some(pattern) = self.pattern_from_effect_ref(pattern)? else {
                    return Ok(None);
                };
                let fact = TraceSpecClauseFact::Allow {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::Allow {
                    pattern,
                    fact,
                    span: expr.span(),
                }]]))
            }
            HirSpecExpr::Allow { pattern, span } => {
                let Some(pattern) = self.pattern_from_effect_ref(pattern)? else {
                    return Ok(None);
                };
                let fact = TraceSpecClauseFact::Allow {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::Allow {
                    pattern,
                    fact,
                    span: *span,
                }]]))
            }
            HirSpecExpr::Deny { pattern, span } => {
                let Some(pattern) = self.pattern_from_effect_ref(pattern)? else {
                    return Ok(None);
                };
                let fact = TraceSpecClauseFact::Deny {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::Deny {
                    pattern,
                    fact,
                    span: *span,
                }]]))
            }
            HirSpecExpr::And { lhs, rhs, .. } => {
                let Some(left) = self.alternatives_from_expr(lhs)? else {
                    return Ok(None);
                };
                let Some(right) = self.alternatives_from_expr(rhs)? else {
                    return Ok(None);
                };
                Ok(Some(conjoin_alternatives(left, right)))
            }
            HirSpecExpr::Or { lhs, rhs, .. } => {
                let Some(mut alternatives) = self.alternatives_from_expr(lhs)? else {
                    return Ok(None);
                };
                let Some(right) = self.alternatives_from_expr(rhs)? else {
                    return Ok(None);
                };
                alternatives.extend(right);
                Ok(Some(alternatives))
            }
            HirSpecExpr::Before {
                before,
                after,
                span,
            } => {
                let Some(guard) = self.pattern_operand(before)? else {
                    return Ok(None);
                };
                let Some(target) = self.pattern_operand(after)? else {
                    return Ok(None);
                };
                let fact = TraceSpecClauseFact::RequireBefore {
                    guard: guard.row.clone(),
                    guard_label: guard.label.clone(),
                    target: target.row.clone(),
                    target_label: target.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::RequireBefore {
                    guard,
                    target,
                    fact,
                    span: *span,
                }]]))
            }
            HirSpecExpr::After {
                after,
                before,
                span,
            } => {
                let Some(target) = self.pattern_operand(before)? else {
                    return Ok(None);
                };
                let Some(obligation) = self.pattern_operand(after)? else {
                    return Ok(None);
                };
                let fact = TraceSpecClauseFact::RequireAfter {
                    target: target.row.clone(),
                    target_label: target.label.clone(),
                    obligation: obligation.row.clone(),
                    obligation_label: obligation.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::RequireAfter {
                    target,
                    obligation,
                    fact,
                    span: *span,
                }]]))
            }
        }
    }

    fn pattern_operand(
        &mut self,
        expr: &HirSpecExpr,
    ) -> Result<Option<TraceSpecPattern>, EffectPipelineError> {
        match expr {
            HirSpecExpr::Atom(pattern)
            | HirSpecExpr::Allow { pattern, .. }
            | HirSpecExpr::Deny { pattern, .. } => self.pattern_from_effect_ref(pattern),
            HirSpecExpr::And { span, .. }
            | HirSpecExpr::Or { span, .. }
            | HirSpecExpr::Before { span, .. }
            | HirSpecExpr::After { span, .. } => {
                self.diagnostic(
                    *span,
                    "temporal trace spec operands must be action patterns",
                );
                Ok(None)
            }
        }
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        self.hir
            .symbols
            .get(symbol)
            .map(|symbol| symbol.name.clone())
            .unwrap_or_else(|| "unknown".to_owned())
    }

    pub(super) fn diagnostic(&mut self, span: Span, message: impl Into<String>) {
        self.effects.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
    }
}

fn callable_symbol_and_conformances(
    item: &HirItem,
) -> Option<(SymbolId, &[etas_hir::HirDeclarationConformance])> {
    match item {
        HirItem::Flow(flow) => Some((flow.symbol, &flow.conformances)),
        HirItem::Tool(tool) => Some((tool.symbol, &tool.conformances)),
        HirItem::Agent(agent) => Some((agent.symbol, &agent.conformances)),
        _ => None,
    }
}

fn effect_label(effect_ref: &HirEffectRef) -> String {
    effect_ref
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn facts_from_alternatives(alternatives: &TraceSpecClauseAlternatives) -> Vec<TraceSpecClauseFact> {
    alternatives
        .iter()
        .flat_map(|clauses| clauses.iter().map(TraceSpecClause::fact))
        .collect()
}

fn conjoin_alternatives(
    left: TraceSpecClauseAlternatives,
    right: TraceSpecClauseAlternatives,
) -> TraceSpecClauseAlternatives {
    let mut out = Vec::new();
    for left_clauses in &left {
        for right_clauses in &right {
            let mut clauses = Vec::with_capacity(left_clauses.len() + right_clauses.len());
            clauses.extend(left_clauses.iter().cloned());
            clauses.extend(right_clauses.iter().cloned());
            out.push(clauses);
        }
    }
    out
}

fn external_row_label(row: &EffectRow, registry: &EffectRegistry) -> String {
    row.effects
        .iter()
        .map(|effect| external_effect_label(effect, registry))
        .collect::<Vec<_>>()
        .join(", ")
}

fn external_effect_label(effect: &Effect, registry: &EffectRegistry) -> String {
    match effect {
        Effect::Tag(tag) => registry
            .tag_name(*tag)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("EffectTag({})", tag.0)),
        Effect::Action(action) => registry
            .action_name(action.tag, action.action)
            .map(|name| {
                format!(
                    "{}.{name}",
                    registry.tag_name(action.tag).unwrap_or("Unknown")
                )
            })
            .unwrap_or_else(|| format!("Action({})", action.action.0)),
        Effect::AppliedAction(action) => {
            let mut label = external_effect_label(&Effect::Action(action.action.clone()), registry);
            label.push('<');
            label.push_str(
                &action
                    .args
                    .iter()
                    .map(|arg| format!("{arg:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            label.push('>');
            label
        }
        Effect::Applied { tag, args } => {
            let mut label = external_effect_label(&Effect::Tag(*tag), registry);
            label.push('<');
            label.push_str(
                &args
                    .iter()
                    .map(|arg| format!("{arg:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            label.push('>');
            label
        }
        Effect::Var(var) => format!("'{}", var.0),
        Effect::Error(error) => format!("Error[{error:?}]"),
    }
}

fn effect_ref_span_for_missing_type(
    hir: &HirProgram,
    ty: etas_hir::HirTypeId,
) -> Result<Span, EffectPipelineError> {
    hir.types
        .get(ty)
        .map(etas_hir::HirType::span)
        .ok_or(EffectPipelineError::MissingTypeFact { ty })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_trace_spec_type_returns_structural_error() {
        let error = effect_ref_span_for_missing_type(&HirProgram::default(), 0_u32.into())
            .expect_err("missing type must fail closed");
        assert_eq!(
            error,
            EffectPipelineError::MissingTypeFact { ty: 0_u32.into() }
        );
    }
}
