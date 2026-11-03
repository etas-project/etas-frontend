use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{HirEffectRef, HirItem, HirItemId, HirProgram, HirSpecExpr, HirSpecKind, SymbolId};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    Effect, EffectOutput, EffectPipelineError, EffectRegistry, EffectRow,
    ExternalTraceSpecSummaryMetadata, RequirementFact, TraceSpecClauseFact,
    trace_spec::TraceSpecModelStore,
    trace_spec::model::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecPattern},
};

mod binding;
mod conformance;
pub(super) mod external;
pub(super) mod instantiate;
mod selector;

pub fn materialize_trace_spec_models(
    hir: &HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    external_trace_specs: &[crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    source_snapshots: &[crate::EffectSourceSnapshot<'_>],
    effects: &mut EffectOutput,
) -> Result<TraceSpecModelStore, EffectPipelineError> {
    let mut checked_snapshots = HashMap::with_capacity(source_snapshots.len());
    for snapshot in source_snapshots {
        let checked = etas_code::residual::identity::CheckedSourceSnapshot::new(
            snapshot.key,
            snapshot.text.as_bytes(),
        );
        if checked_snapshots.insert(snapshot.source, checked).is_some() {
            return Err(EffectPipelineError::InvalidSourceIdentity {
                source: snapshot.source,
                reason: "duplicate source snapshots".into(),
            });
        }
    }
    TraceSpecMaterializer {
        hir,
        types,
        registry,
        external_trace_specs,
        source_snapshots: checked_snapshots,
        source_clause_sites: BTreeMap::new(),
        effects,
    }
    .materialize()
}

pub(super) fn rematerialize_checked_expression(
    hir: &HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    source_snapshot: Option<(
        etas_core::SourceId,
        etas_code::residual::identity::CheckedSourceSnapshot<'_>,
    )>,
    expression: &HirSpecExpr,
) -> Result<TraceSpecClauseAlternatives, EffectPipelineError> {
    let mut checked_snapshots = HashMap::new();
    if let Some((source, snapshot)) = source_snapshot {
        if source != expression.span().source {
            return Err(EffectPipelineError::InvalidSourceIdentity {
                source: expression.span().source,
                reason: "trace expression snapshot has a different source".into(),
            });
        }
        checked_snapshots.insert(source, snapshot);
    }
    let mut effects = EffectOutput::default();
    let alternatives = TraceSpecMaterializer {
        hir,
        types,
        registry,
        external_trace_specs: &[],
        source_snapshots: checked_snapshots,
        source_clause_sites: BTreeMap::new(),
        effects: &mut effects,
    }
    .alternatives_from_expr(expression)?;
    if !effects.diagnostics.is_empty() {
        return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "checked trace expression could not be materialized".into(),
        });
    }
    alternatives.ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
        reason: "checked trace expression has no materialized alternatives".into(),
    })
}

pub(super) fn rematerialize_checked_external(
    hir: &HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    summary: &ExternalTraceSpecSummaryMetadata,
    span: Span,
) -> Result<TraceSpecClauseAlternatives, EffectPipelineError> {
    let mut effects = EffectOutput::default();
    let alternatives = TraceSpecMaterializer {
        hir,
        types,
        registry,
        external_trace_specs: &[],
        source_snapshots: HashMap::new(),
        source_clause_sites: BTreeMap::new(),
        effects: &mut effects,
    }
    .clauses_from_external_summary(summary, span);
    if !effects.diagnostics.is_empty() {
        return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "checked external trace spec could not be materialized".into(),
        });
    }
    alternatives.ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
        reason: "checked external trace spec has no materialized alternatives".into(),
    })
}

/// Reconstruct source instance identities from the checked conformance and
/// immutable source snapshots before any residual lowering can consume them.
pub(super) fn validate_conformance_instances(
    hir: &HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    snapshots: &[crate::EffectSourceSnapshot<'_>],
    models: &TraceSpecModelStore,
    facts: &crate::facts::TraceSpecFacts,
    diagnostics: &[Diagnostic],
) -> Result<(), EffectPipelineError> {
    let max = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    let mut remaining = max;
    let mut checked_snapshots = HashMap::new();
    for snapshot in snapshots {
        remaining = remaining
            .checked_sub(1)
            .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace spec conformance identities",
                limit: max,
            })?;
        let checked = etas_code::residual::identity::CheckedSourceSnapshot::new(
            snapshot.key,
            snapshot.text.as_bytes(),
        );
        if checked_snapshots.insert(snapshot.source, checked).is_some() {
            return Err(EffectPipelineError::InvalidSourceIdentity {
                source: snapshot.source,
                reason: "duplicate source snapshots".into(),
            });
        }
    }
    let mut recorded = HashMap::new();
    for (owner, conformances) in &facts.conformances {
        remaining = remaining
            .checked_sub(1)
            .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace spec conformance identities",
                limit: max,
            })?;
        for conformance in conformances {
            remaining =
                remaining
                    .checked_sub(1)
                    .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace spec conformance identities",
                        limit: max,
                    })?;
            remaining = remaining
                .checked_sub(conformance.alternatives.len())
                .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                    resource: "trace spec conformance identities",
                    limit: max,
                })?;
            for alternative in &conformance.alternatives {
                remaining = remaining.checked_sub(alternative.len()).ok_or(
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace spec conformance identities",
                        limit: max,
                    },
                )?;
            }
            if recorded
                .insert((*owner, conformance.span), conformance)
                .is_some()
            {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "duplicate checked trace conformance facts".into(),
                });
            }
        }
    }
    let mut checked = HashMap::new();
    for (index, fact) in types.facts.trace_spec_conformances.iter().enumerate() {
        remaining = remaining
            .checked_sub(1)
            .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace spec conformance identities",
                limit: max,
            })?;
        if checked.insert((fact.item, fact.span), index).is_some() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "duplicate type-checked trace conformance".into(),
            });
        }
    }
    let mut rejection_diagnostics = HashSet::new();
    for (index, diagnostic_index) in &models.rejected_fact_diagnostics {
        remaining = remaining
            .checked_sub(1)
            .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace spec conformance identities",
                limit: max,
            })?;
        let valid = types.facts.trace_spec_conformances.get(*index).is_some()
            && rejection_diagnostics.insert(*diagnostic_index)
            && diagnostics
                .get(*diagnostic_index)
                .is_some_and(|diagnostic| {
                    diagnostic.severity == etas_core::Severity::Error
                        && diagnostic.code
                            == etas_core::DiagnosticCode::Effect(
                                EffectDiagnosticCode::IncompleteEffectFacts,
                            )
                });
        if !valid {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace conformance rejection has no matching error diagnostic".into(),
            });
        }
    }
    let mut scratch = EffectOutput::default();
    let materializer = TraceSpecMaterializer {
        hir,
        types,
        registry,
        external_trace_specs: &[],
        source_snapshots: checked_snapshots,
        source_clause_sites: BTreeMap::new(),
        effects: &mut scratch,
    };
    if !models
        .referenced_by_item
        .keys()
        .eq(models.referenced_fact_indices.keys())
    {
        return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "trace conformance models and type fact owners do not match".into(),
        });
    }
    let mut used = HashSet::new();
    for (owner, indices) in &models.referenced_fact_indices {
        let alternatives = models.referenced_by_item.get(owner).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "checked conformance alternatives are missing".into(),
            }
        })?;
        if indices.len() != alternatives.len() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "checked conformance alternatives have a different count".into(),
            });
        }
        for (index, expected_alternatives) in indices.iter().zip(alternatives) {
            if models.rejected_fact_diagnostics.contains_key(index) {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace conformance is both rejected and analyzed".into(),
                });
            }
            if !used.insert(*index) {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace conformance model is referenced more than once".into(),
                });
            }
            remaining =
                remaining
                    .checked_sub(1)
                    .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace spec conformance identities",
                        limit: max,
                    })?;
            let fact = types
                .facts
                .trace_spec_conformances
                .get(*index)
                .ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "checked trace conformance index is invalid".into(),
                })?;
            if matches!(
                fact.target,
                etas_types::TraceSpecConformanceTarget::Named { .. }
            ) && materializer.checked_conformance_reference(fact).is_none()
            {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace conformance target differs from its resolved source reference"
                        .into(),
                });
            }
            let stored = recorded.get(&(fact.item, fact.span)).ok_or_else(|| {
                EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "materialized trace conformance fact is missing".into(),
                }
            })?;
            if stored.alternatives != *expected_alternatives
                || stored.source_spec_instance != materializer.checked_source_spec_instance(fact)?
            {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace conformance fact differs from its checked source instance"
                        .into(),
                });
            }
        }
    }
    for (item, span) in recorded.keys() {
        let index = checked.get(&(*item, *span)).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "materialized trace conformance has no checked type fact".into(),
            }
        })?;
        // A bare agent has no callable body to analyze. Invalid default-entry
        // facts already produce a materialization diagnostic.
        let owner = match types.facts.item_signatures.get(item) {
            Some(etas_types::ItemSignature::Agent(agent)) => agent
                .default_entry
                .as_ref()
                .and_then(|entry| entry.method)
                .and_then(|symbol| types.facts.agent_methods.get(&symbol))
                .map(|method| method.body),
            _ => Some(*item),
        };
        if owner
            .and_then(|owner| hir.items.get(owner))
            .and_then(HirItem::symbol)
            .is_some()
            && !used.contains(index)
            && !models.rejected_fact_diagnostics.contains_key(index)
        {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "materialized trace conformance has no analysis model".into(),
            });
        }
    }
    for (index, fact) in types.facts.trace_spec_conformances.iter().enumerate() {
        if used.contains(&index) || models.rejected_fact_diagnostics.contains_key(&index) {
            continue;
        }
        let bare_agent = matches!(types.facts.item_signatures.get(&fact.item),
            Some(etas_types::ItemSignature::Agent(agent)) if agent.default_entry.is_none());
        if !bare_agent || !recorded.contains_key(&(fact.item, fact.span)) {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "checked trace conformance has no model or rejection".into(),
            });
        }
    }
    if !models
        .referenced_by_item
        .keys()
        .eq(models.source_owners.keys())
    {
        return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "trace conformance models and checked source owners do not match".into(),
        });
    }
    remaining.checked_sub(models.source_owners.len()).ok_or(
        EffectPipelineError::AnalysisLimitExceeded {
            resource: "trace spec conformance identities",
            limit: max,
        },
    )?;
    for (owner, stored) in &models.source_owners {
        if *stored != materializer.checked_source_owner(*owner)? {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace source owner differs from its checked declaration".into(),
            });
        }
    }
    Ok(())
}

pub(crate) fn checked_callable_owner_kind(
    hir: &HirProgram,
    owner: HirItemId,
) -> Result<etas_code::residual::identity::ResidualOwnerKind, EffectPipelineError> {
    use etas_code::residual::identity::ResidualOwnerKind;
    let invalid = || EffectPipelineError::InvalidTraceSpecSourceSites {
        reason: "trace conformance owner is not a checked callable".into(),
    };
    match hir.items.get(owner).ok_or_else(invalid)? {
        HirItem::Flow(flow) => match hir.agent_methods.get(&flow.symbol) {
            Some(method)
                if method.symbol == flow.symbol
                    && method.body == owner
                    && method.implementation.is_some() =>
            {
                Ok(ResidualOwnerKind::Method)
            }
            Some(_) => Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace method owner disagrees with checked method ownership".into(),
            }),
            None => Ok(ResidualOwnerKind::Flow),
        },
        HirItem::Agent(_) => Ok(ResidualOwnerKind::Agent),
        HirItem::Tool(_) => Ok(ResidualOwnerKind::Tool),
        _ => Err(invalid()),
    }
}

pub(super) struct TraceSpecMaterializer<'a> {
    pub(super) hir: &'a HirProgram,
    pub(super) types: &'a etas_types::TypeOutput,
    pub(super) registry: &'a EffectRegistry,
    pub(super) external_trace_specs:
        &'a [crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    pub(super) source_snapshots:
        HashMap<etas_core::SourceId, etas_code::residual::identity::CheckedSourceSnapshot<'a>>,
    source_clause_sites: BTreeMap<
        etas_code::residual::identity::ClauseId,
        etas_code::residual::identity::SourceClauseSite,
    >,
    pub(super) effects: &'a mut EffectOutput,
}

impl TraceSpecMaterializer<'_> {
    fn checked_source_owner(
        &self,
        owner: HirItemId,
    ) -> Result<Option<super::model::CheckedTraceSourceOwner>, EffectPipelineError> {
        use etas_code::residual::identity::ResidualOwnerId;
        let declaration = self.hir.items.get(owner).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace conformance owner has no checked declaration".into(),
            }
        })?;
        let kind = checked_callable_owner_kind(self.hir, owner)?;
        let span = declaration.span();
        let Some(snapshot) = self.source_snapshots.get(&span.source) else {
            return Ok(None);
        };
        let location = snapshot
            .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: format!("invalid trace owner source range: {error:?}"),
            })?;
        let id = ResidualOwnerId::from_checked_source_declaration(snapshot, &location, kind)
            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: format!("invalid trace source owner: {error:?}"),
            })?;
        Ok(Some(super::model::CheckedTraceSourceOwner {
            id,
            declaration: location,
            kind,
        }))
    }

    fn source_spec_instance(
        &self,
        span: Span,
        bindings: &[etas_code::residual::identity::StaticBindingRecipe],
    ) -> Result<Option<etas_code::residual::identity::SourceSpecInstance>, EffectPipelineError>
    {
        use etas_code::residual::identity::{
            CheckedSourceStructuralSite, ObligationIdentityLimits, SourceSpecInstance,
        };
        let Some(snapshot) = self.source_snapshots.get(&span.source) else {
            return Ok(None);
        };
        let location = snapshot
            .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: format!("invalid spec declaration source range: {error:?}"),
            })?;
        let site = CheckedSourceStructuralSite {
            location: &location,
            snapshot,
            structural_path: &[],
        };
        SourceSpecInstance::from_checked_declaration(
            &site,
            bindings,
            ObligationIdentityLimits {
                max_generic_arguments:
                    etas_code::residual::identity::MAX_RESIDUAL_REFERENCE_SUBSTITUTIONS,
                max_structural_site_depth: etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
            },
        )
        .map(Some)
        .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
            source: span.source,
            reason: format!("invalid spec declaration source site: {error:?}"),
        })
    }

    fn source_clause(
        &mut self,
        span: Span,
        path: &[u32],
    ) -> Result<Option<etas_code::residual::identity::ClauseId>, EffectPipelineError> {
        use etas_code::residual::identity::{
            CheckedSourceStructuralSite, ObligationIdentityLimits, SourceClauseSite,
        };
        let Some(snapshot) = self.source_snapshots.get(&span.source) else {
            return Ok(None);
        };
        let location = snapshot
            .location(u64::from(span.range.start.0), u64::from(span.range.end.0))
            .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: format!("invalid clause source range: {error:?}"),
            })?;
        let site = CheckedSourceStructuralSite {
            location: &location,
            snapshot,
            structural_path: path,
        };
        let source_site = SourceClauseSite::from_checked_site(
            &site,
            ObligationIdentityLimits {
                max_generic_arguments: 0,
                max_structural_site_depth: etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
            },
        )
        .map_err(|error| EffectPipelineError::InvalidSourceIdentity {
            source: span.source,
            reason: format!("invalid clause source site: {error:?}"),
        })?;
        let id = source_site.id();
        if let Some(previous) = self.source_clause_sites.insert(id, source_site.clone())
            && previous != source_site
        {
            return Err(EffectPipelineError::InvalidSourceIdentity {
                source: span.source,
                reason: "conflicting clause source identities".into(),
            });
        }
        Ok(Some(id))
    }

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
                .declaration_alternatives
                .insert(item, alternatives.clone());
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

        self.materialize_checked_conformances(&mut store)?;
        for conformance in &self.types.facts.external_trace_spec_conformances {
            match &conformance.target {
                etas_types::ExternalTraceSpecConformanceTarget::Named {
                    spec, definition, ..
                } => {
                    let summary = match external_trace_spec_summary(
                        self.external_trace_specs,
                        definition,
                        spec,
                    ) {
                        Ok(Some(summary)) => summary,
                        Ok(None) => {
                            self.diagnostic(
                                conformance.span,
                                format!(
                                    "external trace spec `{}` requires package metadata trace clauses",
                                    spec.join(".")
                                ),
                            );
                            continue;
                        }
                        Err(()) => {
                            self.diagnostic(
                                conformance.span,
                                format!(
                                    "external trace spec `{}` has ambiguous metadata for its definition identity and path",
                                    spec.join(".")
                                ),
                            );
                            continue;
                        }
                    };
                    let Some(alternatives) =
                        self.clauses_from_external_summary(summary, conformance.span)
                    else {
                        continue;
                    };
                    match &conformance.item {
                        etas_package_metadata::CallableContractTarget::Item(path) => {
                            store
                                .external_referenced_by_item
                                .entry(super::model::ExternalTraceSpecItem {
                                    package: conformance.package,
                                    path: path.clone(),
                                })
                                .or_default()
                                .push(alternatives);
                        }
                        etas_package_metadata::CallableContractTarget::Method(identity) => {
                            store
                                .external_referenced_methods
                                .entry(identity.clone())
                                .or_default()
                                .push(alternatives);
                        }
                    }
                }
                etas_types::ExternalTraceSpecConformanceTarget::Inline => {
                    self.diagnostic(
                        conformance.span,
                        "external inline trace spec conformance requires materialized trace spec metadata",
                    );
                }
            }
        }
        let mut used = std::collections::BTreeSet::new();
        for alternatives in store.clauses_by_item.values().chain(
            store
                .referenced_by_item
                .values()
                .chain(store.external_referenced_by_item.values())
                .chain(store.external_referenced_methods.values())
                .flatten(),
        ) {
            used.extend(
                alternatives
                    .iter()
                    .flatten()
                    .filter_map(TraceSpecClause::source_clause),
            );
        }
        self.source_clause_sites.retain(|id, _| used.contains(id));
        store.source_clause_sites = std::mem::take(&mut self.source_clause_sites);
        Ok(store)
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
        self.alternatives_from_expr_at(expr, &[])
    }

    fn alternatives_from_expr_at(
        &mut self,
        expr: &HirSpecExpr,
        source_path: &[u32],
    ) -> Result<Option<TraceSpecClauseAlternatives>, EffectPipelineError> {
        if source_path.len() > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH {
            self.diagnostic(
                expr.span(),
                "trace spec source structure exceeds the depth limit",
            );
            return Ok(None);
        }
        match expr {
            HirSpecExpr::Atom(pattern) => {
                let Some(pattern) = self.pattern_from_effect_ref(pattern)? else {
                    return Ok(None);
                };
                let source_clause = self.source_clause(expr.span(), source_path)?;
                let fact = TraceSpecClauseFact::Allow {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::Allow {
                    pattern,
                    fact,
                    span: expr.span(),
                    source_path: Some(source_path.to_vec()),
                    source_clause,
                }]]))
            }
            HirSpecExpr::Allow { pattern, span } => {
                let Some(pattern) = self.pattern_from_effect_ref(pattern)? else {
                    return Ok(None);
                };
                let source_clause = self.source_clause(expr.span(), source_path)?;
                let fact = TraceSpecClauseFact::Allow {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::Allow {
                    pattern,
                    fact,
                    span: *span,
                    source_path: Some(source_path.to_vec()),
                    source_clause,
                }]]))
            }
            HirSpecExpr::Deny { pattern, span } => {
                let Some(pattern) = self.pattern_from_effect_ref(pattern)? else {
                    return Ok(None);
                };
                let source_clause = self.source_clause(expr.span(), source_path)?;
                let fact = TraceSpecClauseFact::Deny {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
                Ok(Some(vec![vec![TraceSpecClause::Deny {
                    pattern,
                    fact,
                    span: *span,
                    source_path: Some(source_path.to_vec()),
                    source_clause,
                }]]))
            }
            HirSpecExpr::And { lhs, rhs, span } => {
                let mut child = source_path.to_vec();
                child.push(0);
                let Some(left) = self.alternatives_from_expr_at(lhs, &child)? else {
                    return Ok(None);
                };
                child.pop();
                child.push(1);
                let Some(right) = self.alternatives_from_expr_at(rhs, &child)? else {
                    return Ok(None);
                };
                match conjoin_alternatives(left, right) {
                    Some(alternatives) => Ok(Some(alternatives)),
                    None => {
                        self.alternative_limit_diagnostic(*span);
                        Ok(None)
                    }
                }
            }
            HirSpecExpr::Or { lhs, rhs, span } => {
                let mut child = source_path.to_vec();
                child.push(0);
                let Some(mut alternatives) = self.alternatives_from_expr_at(lhs, &child)? else {
                    return Ok(None);
                };
                child.pop();
                child.push(1);
                let Some(right) = self.alternatives_from_expr_at(rhs, &child)? else {
                    return Ok(None);
                };
                if alternatives_size(&alternatives)
                    .and_then(|left| {
                        alternatives_size(&right).and_then(|right| left.checked_add(right))
                    })
                    .is_none_or(|size| size > etas_package_metadata::MAX_METADATA_GRAPH_NODES)
                {
                    self.alternative_limit_diagnostic(*span);
                    return Ok(None);
                }
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
                let source_clause = self.source_clause(expr.span(), source_path)?;
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
                    source_path: Some(source_path.to_vec()),
                    source_clause,
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
                let source_clause = self.source_clause(expr.span(), source_path)?;
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
                    source_path: Some(source_path.to_vec()),
                    source_clause,
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

    fn alternative_limit_diagnostic(&mut self, span: Span) {
        self.diagnostic(
            span,
            "trace spec alternatives exceed the metadata graph node limit",
        );
    }
}

pub(super) fn callable_symbol_and_conformances(
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
) -> Option<TraceSpecClauseAlternatives> {
    let (count, size) = conjunction_size(&left, &right)?;
    if size > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for left_clauses in &left {
        for right_clauses in &right {
            let mut clauses =
                Vec::with_capacity(left_clauses.len().checked_add(right_clauses.len())?);
            clauses.extend(left_clauses.iter().cloned());
            clauses.extend(right_clauses.iter().cloned());
            out.push(clauses);
        }
    }
    Some(out)
}

// Count both alternatives and their clauses before allocating the cross product.
fn alternatives_size<T>(alternatives: &[Vec<T>]) -> Option<usize> {
    alternatives
        .iter()
        .try_fold(alternatives.len(), |size, clauses| {
            size.checked_add(clauses.len())
        })
}

fn external_trace_spec_summary<'a>(
    summaries: &'a [crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    definition: &etas_code::DefinitionKey,
    path: &[String],
) -> Result<Option<&'a crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>>, ()> {
    let mut matches = summaries
        .iter()
        .filter(|summary| summary.definition == *definition);
    let first = matches.next();
    let Some(first) = first else {
        return Ok(None);
    };
    let Some(second) = matches.next() else {
        return Ok(Some(first));
    };
    let mut selected = None;
    for candidate in std::iter::once(first)
        .chain(std::iter::once(second))
        .chain(matches)
    {
        if candidate.trace_spec == path && selected.replace(candidate).is_some() {
            return Err(());
        }
    }
    selected.map(Some).ok_or(())
}

fn conjunction_size<T>(left: &[Vec<T>], right: &[Vec<T>]) -> Option<(usize, usize)> {
    let count = left.len().checked_mul(right.len())?;
    let left_clauses = alternatives_size(left)?.checked_sub(left.len())?;
    let right_clauses = alternatives_size(right)?.checked_sub(right.len())?;
    let size = count
        .checked_add(left_clauses.checked_mul(right.len())?)?
        .checked_add(right_clauses.checked_mul(left.len())?)?;
    Some((count, size))
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
    fn agent_method_trace_owner_has_method_identity() {
        use etas_code::{CodeFingerprint, StableSourceKey, StableSourceOrigin};
        use etas_core::SourceId;
        let source = "module app; effect Gate { action a<T>(value: T) -> unit; } spec Safe<T>: trace = -Gate.a<T>; agent Worker; impl agent Worker { flow helper<T>(value: T) -> unit ![] ~ Safe<T> { return; } }";
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            source.to_owned(),
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::lower_program(&parsed.value);
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let registry = EffectRegistry::from_hir_and_types(&hir, &types).unwrap();
        let key = StableSourceKey::new(
            StableSourceOrigin::Project(CodeFingerprint::from_bytes([46; 32])),
            &["src".into(), "main.es".into()],
        )
        .unwrap();
        let snapshots = [crate::EffectSourceSnapshot {
            source: SourceId(0),
            key,
            text: source,
        }];
        let mut effects = EffectOutput::default();
        let models =
            materialize_trace_spec_models(&hir, &types, &registry, &[], &snapshots, &mut effects)
                .unwrap();
        let method = hir
            .agent_methods
            .values()
            .find(|method| method.implementation.is_some())
            .unwrap();
        let owner = models
            .source_owners
            .get(&method.body)
            .and_then(Option::as_ref)
            .expect("method trace owner");
        assert_eq!(
            owner.kind(),
            etas_code::residual::identity::ResidualOwnerKind::Method
        );
        let instance = effects.facts.trace_specs.conformances[&method.body][0]
            .source_spec_instance
            .as_ref()
            .expect("source spec instance");
        assert_eq!(
            instance.arguments(),
            &[
                etas_code::residual::identity::StaticBindingRecipe::TypeParameter(
                    etas_code::residual::identity::GenericParameterId::from_source_owner(
                        owner.id(),
                        0,
                        etas_code::descriptor::GenericParameterKind::Type,
                    ),
                )
            ]
        );
        let mut corrupt = hir.clone();
        corrupt.agent_methods.get_mut(&method.symbol).unwrap().body = etas_hir::HirItemId(u32::MAX);
        assert!(matches!(
            checked_callable_owner_kind(&corrupt, method.body),
            Err(EffectPipelineError::InvalidTraceSpecSourceSites { .. })
        ));
    }

    #[test]
    fn source_owner_must_match_checked_callable_and_snapshot() {
        use etas_code::{CodeFingerprint, StableSourceKey, StableSourceOrigin};
        use etas_core::SourceId;

        let source = "module app; effect Gate { action a() -> unit; } spec Safe: trace = -Gate.a; flow main() -> unit ![] ~ Safe { return; }";
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
            SourceId(0),
            None,
            source.to_owned(),
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::lower_program(&parsed.value);
        let revision = etas_hir::HirRevision::try_new(hir.clone()).unwrap();
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let registry = EffectRegistry::from_hir_and_types(&hir, &types).unwrap();
        let key = StableSourceKey::new(
            StableSourceOrigin::Project(CodeFingerprint::from_bytes([46; 32])),
            &["src".into(), "main.es".into()],
        )
        .unwrap();
        let snapshots = [crate::EffectSourceSnapshot {
            source: SourceId(0),
            key,
            text: source,
        }];
        let mut effects = EffectOutput::default();
        let models =
            materialize_trace_spec_models(&hir, &types, &registry, &[], &snapshots, &mut effects)
                .unwrap();
        assert!(
            !models.source_owners.is_empty(),
            "types={:?}; diagnostics={:?}",
            types.facts.trace_spec_conformances,
            effects.diagnostics
        );
        assert!(models.source_owners.values().flatten().all(|owner| owner.kind() == etas_code::residual::identity::ResidualOwnerKind::Flow));
        models
            .validate_source_sites(&revision, &types, &registry, &[], &snapshots)
            .unwrap();
        let mut missing = models.clone();
        *missing.source_owners.values_mut().next().unwrap() = None;
        assert!(matches!(
            validate_conformance_instances(
                &hir,
                &types,
                &registry,
                &snapshots,
                &missing,
                &effects.facts.trace_specs,
                &effects.diagnostics
            ),
            Err(EffectPipelineError::InvalidTraceSpecSourceSites { .. })
        ));
        let mut forged = models.clone();
        forged
            .source_owners
            .values_mut()
            .next()
            .unwrap()
            .as_mut()
            .unwrap()
            .id = etas_code::residual::identity::ResidualOwnerId::from_canonical_fingerprint(
            CodeFingerprint::from_bytes([47; 32]),
        );
        assert!(matches!(
            validate_conformance_instances(
                &hir,
                &types,
                &registry,
                &snapshots,
                &forged,
                &effects.facts.trace_specs,
                &effects.diagnostics
            ),
            Err(EffectPipelineError::InvalidTraceSpecSourceSites { .. })
        ));
    }

    #[test]
    fn external_trace_spec_selection_uses_definition_identity() {
        use etas_core::{SourceId, TextRange, TextSize};

        let path = vec!["dep".to_owned(), "Safe".to_owned()];
        let key = |package| {
            etas_code::DefinitionKey::new(
                etas_code::PackageKey::from_canonical_fingerprint(
                    etas_code::CodeFingerprint::from_bytes([package; 32]),
                ),
                vec!["dep".into()],
                vec!["Safe".into()],
                etas_code::DefinitionKind::Spec,
            )
            .unwrap()
        };
        let summary = |package| crate::AnchoredExternalMetadata {
            package: u32::from(package),
            metadata: ExternalTraceSpecSummaryMetadata {
                trace_spec: path.clone(),
                definition: key(package),
                alternatives: Vec::new(),
            },
            span: Span::new(SourceId(1), TextRange::new(TextSize(0), TextSize(1))),
        };
        let summaries = [summary(1), summary(2)];
        assert_eq!(
            external_trace_spec_summary(&summaries, &key(2), &path)
                .expect("unambiguous")
                .expect("present")
                .package,
            2
        );
        assert!(
            external_trace_spec_summary(&summaries, &key(3), &path)
                .expect("unambiguous")
                .is_none()
        );
        let mut alias = summary(1);
        alias.package = 2;
        alias.metadata.trace_spec = vec!["other".into(), "Safe".into()];
        let aliases = [summary(1), alias];
        assert_eq!(
            external_trace_spec_summary(&aliases, &key(1), &path)
                .unwrap()
                .unwrap()
                .package,
            1
        );
        let mut conflicting = summary(1);
        conflicting.metadata.alternatives.push(Vec::new());
        let duplicates = [summary(1), conflicting];
        assert!(external_trace_spec_summary(&duplicates, &key(1), &path).is_err());

        let mut types = etas_types::TypeOutput::default();
        std::sync::Arc::make_mut(&mut types.facts)
            .external_trace_spec_conformances
            .push(etas_types::ExternalTraceSpecConformanceFact {
                package: 1,
                item: vec!["dep".into(), "f".into()].into(),
                target: etas_types::ExternalTraceSpecConformanceTarget::Named {
                    spec: path,
                    definition: key(1),
                    args: Vec::new(),
                },
                span: summaries[0].span,
            });
        let mut output = EffectOutput::default();
        let models = materialize_trace_spec_models(
            &HirProgram::default(),
            &types,
            &EffectRegistry::with_standard_effects(),
            &duplicates,
            &[],
            &mut output,
        )
        .expect("materialization must return a diagnostic, not panic");
        assert!(models.external_referenced_by_item.is_empty());
        assert!(output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code
                == etas_core::DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)
                && diagnostic.message.contains("ambiguous metadata")
        }));
    }

    #[test]
    fn missing_trace_spec_type_returns_structural_error() {
        let error = effect_ref_span_for_missing_type(&HirProgram::default(), 0_u32.into())
            .expect_err("missing type must fail closed");
        assert_eq!(
            error,
            EffectPipelineError::MissingTypeFact { ty: 0_u32.into() }
        );
    }

    #[test]
    fn conjunction_budget_counts_all_copied_clauses() {
        let left = vec![vec![1, 2], vec![3]];
        let right = vec![vec![4], vec![5, 6], vec![7, 8, 9]];
        assert_eq!(alternatives_size(&left), Some(5));
        assert_eq!(conjunction_size(&left, &right), Some((6, 27)));
    }

    #[test]
    fn conjunction_rejects_expansion_before_building_it() {
        let clause = TraceSpecClause::Limit {
            fact: TraceSpecClauseFact::Limit { count: 1 },
        };
        let alternatives = vec![vec![clause]; 200];
        assert!(conjoin_alternatives(alternatives.clone(), alternatives).is_none());
    }
}
