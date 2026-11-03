use std::collections::{BTreeSet, HashMap, HashSet};

use etas_code::residual::identity::{CheckedSourceSnapshot, ObligationIdentityLimits};

use super::model::{TraceSpecClause, TraceSpecModelStore};
use crate::{EffectPipelineError, EffectRegistry, EffectSourceSnapshot};

struct SourceSnapshots<'a> {
    inputs: HashMap<etas_core::SourceId, EffectSourceSnapshot<'a>>,
    checked: HashMap<etas_core::SourceId, CheckedSourceSnapshot<'a>>,
}

impl<'a> SourceSnapshots<'a> {
    fn new(snapshots: &[EffectSourceSnapshot<'a>]) -> Result<Self, EffectPipelineError> {
        crate::pipeline::validate_source_snapshot_identities(snapshots, None)?;
        let mut inputs = HashMap::new();
        for snapshot in snapshots {
            inputs.insert(snapshot.source, *snapshot);
        }
        Ok(Self {
            inputs,
            checked: HashMap::new(),
        })
    }

    fn snapshot(&mut self, source: etas_core::SourceId) -> Option<CheckedSourceSnapshot<'a>> {
        let input = self.inputs.get(&source)?;
        Some(
            *self
                .checked
                .entry(source)
                .or_insert_with(|| CheckedSourceSnapshot::new(input.key, input.text.as_bytes())),
        )
    }
}

struct CheckedInputs<'a, 's> {
    hir: &'a etas_hir::HirRevision,
    types: &'a etas_types::TypeOutput,
    registry: &'a EffectRegistry,
    external_trace_specs:
        &'a [crate::AnchoredExternalMetadata<crate::ExternalTraceSpecSummaryMetadata>],
    sources: &'a mut SourceSnapshots<'s>,
    max: usize,
}

impl CheckedInputs<'_, '_> {
    fn expression(
        &mut self,
        expression: &etas_hir::HirSpecExpr,
        remaining: &mut usize,
    ) -> Result<super::model::TraceSpecClauseAlternatives, EffectPipelineError> {
        let source = expression.span().source;
        let snapshot = self
            .sources
            .snapshot(source)
            .map(|snapshot| (source, snapshot));
        let alternatives = super::materialize::rematerialize_checked_expression(
            self.hir,
            self.types,
            self.registry,
            snapshot,
            expression,
        )?;
        charge_alternatives(remaining, &alternatives, self.max)?;
        Ok(alternatives)
    }
}

impl TraceSpecModelStore {
    /// Validates descriptive source provenance; this does not establish plan
    /// coverage, runtime support or installation.
    pub fn validate_source_sites(
        &self,
        hir: &etas_hir::HirRevision,
        types: &etas_types::TypeOutput,
        registry: &EffectRegistry,
        external_trace_specs: &[crate::AnchoredExternalMetadata<
            crate::ExternalTraceSpecSummaryMetadata,
        >],
        snapshots: &[EffectSourceSnapshot<'_>],
    ) -> Result<(), EffectPipelineError> {
        let max = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
        if snapshots.len() > max
            || external_trace_specs.len() > max
            || self.source_clause_sites.len() > max
            || self.clauses_by_item.len() > max
            || self.referenced_by_item.len() > max
            || self.external_referenced_by_item.len() > max
        {
            return Err(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace spec source sites",
                limit: max,
            });
        }
        let mut remaining = max;
        for alternatives in self.clauses_by_item.values().chain(
            self.referenced_by_item
                .values()
                .chain(self.external_referenced_by_item.values())
                .flatten(),
        ) {
            charge_alternatives(&mut remaining, alternatives, max)?;
        }
        let mut sources = SourceSnapshots::new(snapshots)?;
        {
            let mut inputs = CheckedInputs {
                hir,
                types,
                registry,
                external_trace_specs,
                sources: &mut sources,
                max,
            };
            self.validate_checked_declarations(&mut inputs, &mut remaining)?;
            self.validate_checked_conformances(&mut inputs, &mut remaining)?;
        }
        let local_spans = local_clause_spans(hir, &sources.inputs, max)?;
        let mut used = BTreeSet::new();
        for (alternatives, source_declaration) in self
            .clauses_by_item
            .values()
            .map(|alternatives| (alternatives, true))
            .chain(
                self.referenced_by_item
                    .values()
                    .chain(self.external_referenced_by_item.values())
                    .flatten()
                    .map(|alternatives| (alternatives, false)),
            )
        {
            for clause in alternatives.iter().flatten() {
                let (span, path) = match clause {
                    TraceSpecClause::Allow {
                        span, source_path, ..
                    }
                    | TraceSpecClause::Deny {
                        span, source_path, ..
                    }
                    | TraceSpecClause::RequireBefore {
                        span, source_path, ..
                    }
                    | TraceSpecClause::RequireAfter {
                        span, source_path, ..
                    } => (*span, source_path),
                    TraceSpecClause::Limit { .. } => continue,
                };
                let Some(id) = clause.source_clause() else {
                    if (source_declaration || path.is_some() || local_spans.contains(&span))
                        && sources.inputs.contains_key(&span.source)
                    {
                        return Err(EffectPipelineError::InvalidSourceIdentity {
                            source: span.source,
                            reason: "source clause is missing its stable identity".into(),
                        });
                    }
                    continue;
                };
                let invalid = |reason: &str| EffectPipelineError::InvalidSourceIdentity {
                    source: span.source,
                    reason: reason.into(),
                };
                let site = self
                    .source_clause_sites
                    .get(&id)
                    .ok_or_else(|| invalid("clause source site is missing"))?;
                if site.id() != id
                    || path.as_deref() != Some(site.structural_path())
                    || site.location().byte_start != u64::from(span.range.start.0)
                    || site.location().byte_end != u64::from(span.range.end.0)
                {
                    return Err(invalid("clause source site does not match the model"));
                }
                let snapshot = sources
                    .snapshot(span.source)
                    .ok_or_else(|| invalid("clause source snapshot is missing"))?;
                site.validate_snapshot(
                    &snapshot,
                    ObligationIdentityLimits {
                        max_generic_arguments: 0,
                        max_structural_site_depth: etas_package_metadata::MAX_METADATA_GRAPH_DEPTH,
                    },
                )
                .map_err(|_| invalid("clause source site does not match its snapshot"))?;
                used.insert(id);
            }
        }
        if used.len() != self.source_clause_sites.len() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "unreferenced clause source site".into(),
            });
        }
        Ok(())
    }

    fn validate_checked_declarations(
        &self,
        input: &mut CheckedInputs<'_, '_>,
        remaining: &mut usize,
    ) -> Result<(), EffectPipelineError> {
        for (item, actual) in &self.clauses_by_item {
            let Some(etas_hir::HirItem::Spec(spec)) = input.hir.items.get(*item) else {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace spec model has no checked declaration".into(),
                });
            };
            if !matches!(spec.kind, etas_hir::HirSpecKind::TraceSpec) {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace spec model points to a different spec kind".into(),
                });
            }
            let expression = spec.trace.as_ref().ok_or_else(|| {
                EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace spec declaration has no checked expression".into(),
                }
            })?;
            if input.expression(expression, remaining)? != *actual {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace spec model differs from its checked declaration".into(),
                });
            }
        }
        Ok(())
    }

    fn validate_checked_conformances(
        &self,
        input: &mut CheckedInputs<'_, '_>,
        remaining: &mut usize,
    ) -> Result<(), EffectPipelineError> {
        let hir = input.hir;
        let types = input.types;
        let max = input.max;
        let invalid = |reason: &str| EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: reason.into(),
        };
        if self.referenced_by_item.len() != self.referenced_fact_indices.len() {
            return Err(invalid("conformance fact owners do not match the model"));
        }
        let mut seen = BTreeSet::new();
        for (owner, conformances) in &self.referenced_by_item {
            let indices = self
                .referenced_fact_indices
                .get(owner)
                .ok_or_else(|| invalid("conformance fact bindings are missing"))?;
            if indices.len() != conformances.len() {
                return Err(invalid("conformance fact count does not match the model"));
            }
            for (index, actual) in indices.iter().zip(conformances) {
                if seen.len() >= max || !seen.insert(*index) {
                    return Err(invalid(
                        "conformance fact binding is duplicated or exceeds the limit",
                    ));
                }
                let fact = types
                    .facts
                    .trace_spec_conformances
                    .get(*index)
                    .ok_or_else(|| invalid("conformance fact binding is invalid"))?;
                let expected_owner = match types.facts.item_signatures.get(&fact.item) {
                    Some(etas_types::ItemSignature::Agent(agent)) => {
                        let entry = agent
                            .default_entry
                            .as_ref()
                            .and_then(|entry| entry.method)
                            .and_then(|method| types.facts.agent_methods.get(&method))
                            .ok_or_else(|| {
                                invalid("agent conformance has no checked default method")
                            })?;
                        entry.body
                    }
                    _ => fact.item,
                };
                if *owner != expected_owner {
                    return Err(invalid("conformance fact has a different callable owner"));
                }
                let (spec_symbol, args) = match &fact.target {
                    etas_types::TraceSpecConformanceTarget::Named { spec_symbol, args } => {
                        (spec_symbol, args)
                    }
                    etas_types::TraceSpecConformanceTarget::Inline => {
                        let expression = hir
                            .items
                            .get(fact.item)
                            .and_then(super::materialize::callable_symbol_and_conformances)
                            .and_then(|(_, conformances)| {
                                conformances.iter().find_map(|conformance| {
                                    match &conformance.target {
                                    etas_hir::HirDeclarationConformanceTarget::InlineTraceSpec(expr)
                                        if expr.span() == fact.span => Some(expr),
                                    _ => None,
                                }
                                })
                            })
                            .ok_or_else(|| {
                                invalid("inline conformance has no matching checked expression")
                            })?;
                        if input.expression(expression, remaining)? != *actual {
                            return Err(invalid(
                                "inline conformance differs from its checked expression",
                            ));
                        }
                        continue;
                    }
                };
                let spec_item = hir.symbols.get(*spec_symbol)
                    .and_then(|symbol| symbol.defining_item)
                    .filter(|item| matches!(hir.items.get(*item), Some(etas_hir::HirItem::Spec(spec)) if matches!(spec.kind, etas_hir::HirSpecKind::TraceSpec)));
                let signature = types
                    .facts
                    .spec_signatures
                    .get(spec_symbol)
                    .filter(|signature| signature.kind == etas_types::SpecKind::TraceSpec)
                    .ok_or_else(|| invalid("checked trace spec signature is missing"))?;
                let mut expected = if let Some(spec_item) = spec_item {
                    let expected = self
                        .clauses_by_item
                        .get(&spec_item)
                        .ok_or_else(|| invalid("source spec declaration is missing"))?;
                    charge_alternatives(remaining, expected, max)?;
                    expected.clone()
                } else {
                    if !matches!(
                        hir.symbols.get(*spec_symbol).map(|symbol| &symbol.def),
                        Some(etas_hir::SymbolDef::ImportAlias { .. })
                    ) {
                        return Err(invalid("external trace spec has no checked import symbol"));
                    }
                    let summary = super::materialize::external::checked_external_summary(
                        types,
                        *spec_symbol,
                        input.external_trace_specs,
                    )
                    .map_err(invalid)?;
                    let mut size = 1usize;
                    size = size
                        .checked_add(summary.metadata.alternatives.len())
                        .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                            resource: "trace spec conformance verification",
                            limit: max,
                        })?;
                    for alternative in &summary.metadata.alternatives {
                        size = size.checked_add(alternative.len()).ok_or(
                            EffectPipelineError::AnalysisLimitExceeded {
                                resource: "trace spec conformance verification",
                                limit: max,
                            },
                        )?;
                    }
                    *remaining = remaining.checked_sub(size).ok_or(
                        EffectPipelineError::AnalysisLimitExceeded {
                            resource: "trace spec conformance verification",
                            limit: max,
                        },
                    )?;
                    super::materialize::rematerialize_checked_external(
                        hir,
                        types,
                        input.registry,
                        &summary.metadata,
                        fact.span,
                    )?
                };
                super::materialize::instantiate::instantiate(
                    &mut expected,
                    signature,
                    args,
                    &types.store,
                )
                .map_err(|_| invalid("trace spec instantiation is invalid"))?;
                if &expected != actual {
                    return Err(invalid(
                        "trace spec conformance differs from its checked target",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn charge_alternatives(
    remaining: &mut usize,
    alternatives: &super::model::TraceSpecClauseAlternatives,
    max: usize,
) -> Result<(), EffectPipelineError> {
    let exceeded = || EffectPipelineError::AnalysisLimitExceeded {
        resource: "trace spec conformance verification",
        limit: max,
    };
    *remaining = remaining.checked_sub(1).ok_or_else(exceeded)?;
    *remaining = remaining
        .checked_sub(alternatives.len())
        .ok_or_else(exceeded)?;
    for alternative in alternatives {
        *remaining = remaining
            .checked_sub(alternative.len())
            .ok_or_else(exceeded)?;
    }
    Ok(())
}

fn local_clause_spans(
    hir: &etas_hir::HirRevision,
    inputs: &HashMap<etas_core::SourceId, EffectSourceSnapshot<'_>>,
    max: usize,
) -> Result<HashSet<etas_core::Span>, EffectPipelineError> {
    use etas_hir::{HirDeclarationConformanceTarget, HirItem, HirSpecExpr, HirSpecKind};

    let mut spans = HashSet::new();
    let mut remaining = max;
    let mut collect = |root: &HirSpecExpr| -> Result<(), EffectPipelineError> {
        if !inputs.contains_key(&root.span().source) {
            return Ok(());
        }
        let mut pending = vec![root];
        while let Some(expr) = pending.pop() {
            remaining =
                remaining
                    .checked_sub(1)
                    .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace spec source expressions",
                        limit: max,
                    })?;
            match expr {
                HirSpecExpr::And { lhs, rhs, .. } | HirSpecExpr::Or { lhs, rhs, .. } => {
                    pending.push(rhs);
                    pending.push(lhs);
                }
                _ => {
                    spans.insert(expr.span());
                }
            }
        }
        Ok(())
    };
    for (_, item) in hir.items.iter() {
        if let HirItem::Spec(spec) = item
            && matches!(spec.kind, HirSpecKind::TraceSpec)
            && let Some(expr) = &spec.trace
        {
            collect(expr)?;
        }
        let conformances = match item {
            HirItem::Flow(flow) => &flow.conformances,
            HirItem::Tool(tool) => &tool.conformances,
            HirItem::Agent(agent) => &agent.conformances,
            _ => continue,
        };
        for conformance in conformances {
            if let HirDeclarationConformanceTarget::InlineTraceSpec(expr) = &conformance.target {
                collect(expr)?;
            }
        }
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_site_validation_rejects_duplicate_stable_keys() {
        let key = etas_code::StableSourceKey::new(
            etas_code::StableSourceOrigin::Project(etas_code::CodeFingerprint::from_bytes([7; 32])),
            &["main.es".into()],
        )
        .unwrap();
        let hir = etas_hir::HirRevision::try_new(etas_hir::HirProgram::default()).unwrap();
        let types = etas_types::TypeOutput::default();
        let registry = EffectRegistry::with_standard_effects();
        let models = TraceSpecModelStore::default();
        for second_text in ["same", "different"] {
            let sources = [
                EffectSourceSnapshot {
                    source: etas_core::SourceId(1),
                    key,
                    text: "same",
                },
                EffectSourceSnapshot {
                    source: etas_core::SourceId(2),
                    key,
                    text: second_text,
                },
            ];
            assert!(matches!(
                SourceSnapshots::new(&sources),
                Err(EffectPipelineError::InvalidSourceIdentity {
                    source: etas_core::SourceId(2),
                    ..
                })
            ));
            assert!(matches!(
                models.validate_source_sites(&hir, &types, &registry, &[], &sources),
                Err(EffectPipelineError::InvalidSourceIdentity {
                    source: etas_core::SourceId(2),
                    ..
                })
            ));
        }
        let other = etas_code::StableSourceKey::new(
            etas_code::StableSourceOrigin::Project(etas_code::CodeFingerprint::from_bytes([7; 32])),
            &["other.es".into()],
        )
        .unwrap();
        let distinct = [
            EffectSourceSnapshot {
                source: etas_core::SourceId(1),
                key,
                text: "same",
            },
            EffectSourceSnapshot {
                source: etas_core::SourceId(2),
                key: other,
                text: "same",
            },
        ];
        assert!(SourceSnapshots::new(&distinct).is_ok());
        assert!(
            models
                .validate_source_sites(&hir, &types, &registry, &[], &distinct)
                .is_ok()
        );
    }

    #[test]
    fn conformance_budget_counts_empty_alternatives_before_copying() {
        let alternatives = vec![vec![], vec![]];
        assert!(matches!(
            charge_alternatives(&mut 2, &alternatives, 2),
            Err(EffectPipelineError::AnalysisLimitExceeded { .. })
        ));
        let mut remaining = 3;
        assert!(charge_alternatives(&mut remaining, &alternatives, 3).is_ok());
        assert_eq!(remaining, 0);
    }
}
