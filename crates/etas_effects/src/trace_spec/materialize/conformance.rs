use super::{TraceSpecMaterializer, callable_symbol_and_conformances, facts_from_alternatives};
use crate::trace_spec::model::TraceSpecModelStore;
use crate::{EffectPipelineError, TraceSpecClauseFact};
use etas_hir::{HirDeclarationConformanceTarget, HirItem};

impl TraceSpecMaterializer<'_> {
    pub(super) fn materialize_checked_conformances(
        &mut self,
        store: &mut TraceSpecModelStore,
    ) -> Result<(), EffectPipelineError> {
        self.validate_conformance_fact_coverage();
        let types = self.types;
        for (fact_index, fact) in types.facts.trace_spec_conformances.iter().enumerate() {
            let diagnostics_before = self.effects.diagnostics.len();
            let (reference, alternatives) = match &fact.target {
                etas_types::TraceSpecConformanceTarget::Named { spec_symbol, args } => {
                    let Some(signature) = types
                        .facts
                        .spec_signatures
                        .get(spec_symbol)
                        .filter(|signature| signature.kind == etas_types::SpecKind::TraceSpec)
                    else {
                        self.diagnostic(
                            fact.span,
                            "trace conformance requires its checked trace spec signature",
                        );
                        self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                        continue;
                    };
                    let Some((name, mut alternatives)) =
                        self.trace_spec_clauses_for_symbol(*spec_symbol, fact.span, store)
                    else {
                        self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                        continue;
                    };
                    if let Err(error) = super::instantiate::instantiate(
                        &mut alternatives,
                        signature,
                        args,
                        &types.store,
                    ) {
                        self.diagnostic(
                            fact.span,
                            format!("trace spec argument instantiation is incomplete: {error}"),
                        );
                        self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                        continue;
                    }
                    (
                        Some(TraceSpecClauseFact::TraceSpecReference { name }),
                        alternatives,
                    )
                }
                etas_types::TraceSpecConformanceTarget::Inline => {
                    let expression = self
                        .hir
                        .items
                        .get(fact.item)
                        .and_then(callable_symbol_and_conformances)
                        .and_then(|(_, conformances)| {
                            conformances
                                .iter()
                                .find_map(|conformance| match &conformance.target {
                                    HirDeclarationConformanceTarget::InlineTraceSpec(expr)
                                        if expr.span() == fact.span =>
                                    {
                                        Some(expr)
                                    }
                                    _ => None,
                                })
                        });
                    let Some(expression) = expression else {
                        self.diagnostic(
                            fact.span,
                            "inline trace conformance has no matching checked HIR expression",
                        );
                        self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                        continue;
                    };
                    let Some(alternatives) = self.alternatives_from_expr(expression)? else {
                        self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                        continue;
                    };
                    (None, alternatives)
                }
            };
            let source_spec_instance = self.checked_source_spec_instance(fact)?;
            self.effects
                .facts
                .trace_specs
                .conformances
                .entry(fact.item)
                .or_default()
                .push(crate::facts::TraceSpecConformanceModelFact {
                    span: fact.span,
                    source_spec_instance,
                    alternatives: alternatives.clone(),
                });
            let mut owner = fact.item;
            if let Some(etas_types::ItemSignature::Agent(agent)) =
                types.facts.item_signatures.get(&owner)
            {
                let Some(entry) = &agent.default_entry else {
                    continue;
                };
                let Some(method) = entry
                    .method
                    .and_then(|symbol| types.facts.agent_methods.get(&symbol))
                else {
                    self.diagnostic(
                        fact.span,
                        "agent trace conformance requires its checked default method",
                    );
                    self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                    continue;
                };
                owner = method.body;
            }
            let Some(symbol) = self.hir.items.get(owner).and_then(HirItem::symbol) else {
                self.diagnostic(
                    fact.span,
                    "trace conformance has no checked callable body owner",
                );
                self.reject_conformance(store, fact_index, diagnostics_before, fact.span);
                continue;
            };
            let source_owner = self.checked_source_owner(owner)?;
            if let Some(previous) = store.source_owners.insert(owner, source_owner.clone())
                && previous != source_owner
            {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace conformance has conflicting source owners".into(),
                });
            }
            let mut facts = Vec::new();
            if let Some(reference) = reference {
                facts.push(reference.clone());
                self.record_trace_spec_requirement(owner, symbol, reference);
            }
            facts.extend(facts_from_alternatives(&alternatives));
            self.effects
                .facts
                .trace_specs
                .items
                .entry(owner)
                .or_default()
                .extend(facts.iter().cloned());
            self.effects
                .facts
                .trace_specs
                .symbols
                .entry(symbol)
                .or_default()
                .extend(facts);
            store
                .referenced_by_item
                .entry(owner)
                .or_default()
                .push(alternatives);
            store
                .referenced_fact_indices
                .entry(owner)
                .or_default()
                .push(fact_index);
        }
        Ok(())
    }

    fn reject_conformance(
        &mut self,
        store: &mut TraceSpecModelStore,
        fact_index: usize,
        diagnostics_before: usize,
        span: etas_core::Span,
    ) {
        if self.effects.diagnostics.len() == diagnostics_before {
            self.diagnostic(span, "trace conformance could not be materialized");
        }
        store
            .rejected_fact_diagnostics
            .insert(fact_index, self.effects.diagnostics.len() - 1);
    }

    pub(super) fn checked_source_spec_instance(
        &self,
        fact: &etas_types::TraceSpecConformanceFact,
    ) -> Result<Option<etas_code::residual::identity::SourceSpecInstance>, EffectPipelineError>
    {
        match &fact.target {
            etas_types::TraceSpecConformanceTarget::Named { spec_symbol, args } => {
                let Some(signature) = self.types.facts.spec_signatures.get(spec_symbol) else {
                    return Ok(None);
                };
                if signature.params.len() != args.len() {
                    return Ok(None);
                }
                let Some(HirItem::Spec(spec)) = self
                    .hir
                    .symbols
                    .get(*spec_symbol)
                    .and_then(|symbol| symbol.defining_item)
                    .and_then(|item| self.hir.items.get(item))
                else {
                    return Ok(None);
                };
                let Some(bindings) = self.source_spec_bindings(fact, args)? else {
                    return Ok(None);
                };
                self.source_spec_instance(spec.span, &bindings)
            }
            etas_types::TraceSpecConformanceTarget::Inline => {
                let expression = self
                    .hir
                    .items
                    .get(fact.item)
                    .and_then(callable_symbol_and_conformances)
                    .and_then(|(_, conformances)| {
                        conformances
                            .iter()
                            .find_map(|conformance| match &conformance.target {
                                HirDeclarationConformanceTarget::InlineTraceSpec(expr)
                                    if expr.span() == fact.span =>
                                {
                                    Some(expr)
                                }
                                _ => None,
                            })
                    });
                match expression {
                    Some(expression) => self.source_spec_instance(expression.span(), &[]),
                    None => Ok(None),
                }
            }
        }
    }

    fn validate_conformance_fact_coverage(&mut self) {
        let hir = self.hir;
        for (item, declaration) in hir.items.iter() {
            if let Some((_, conformances)) = callable_symbol_and_conformances(declaration) {
                for conformance in conformances {
                    match &conformance.target {
                        HirDeclarationConformanceTarget::Path(reference) => {
                            self.require_named_conformance(item, reference)
                        }
                        HirDeclarationConformanceTarget::InlineTraceSpec(expression) => {
                            if !self.types.facts.trace_spec_conformances.iter().any(|fact| {
                                fact.item == item
                                    && fact.span == expression.span()
                                    && matches!(
                                        fact.target,
                                        etas_types::TraceSpecConformanceTarget::Inline
                                    )
                            }) {
                                self.diagnostic(
                                    expression.span(),
                                    "inline trace spec conformance requires checked type facts",
                                );
                            }
                        }
                        HirDeclarationConformanceTarget::Error { .. } => {}
                    }
                }
            }
            if let HirItem::Impl(implementation) = declaration
                && let etas_hir::HirImplTarget::Agent { specs, .. } = &implementation.target
            {
                for reference in specs {
                    if self.trace_spec_symbol(reference).is_none() {
                        continue;
                    }
                    let methods = self
                        .types
                        .facts
                        .agent_methods
                        .values()
                        .filter(|method| {
                            method.implementation == Some(item) && method.name == "run"
                        })
                        .collect::<Vec<_>>();
                    match methods.as_slice() {
                        [entry] => self.require_named_conformance(entry.body, reference),
                        _ => self.diagnostic(
                            reference.span,
                            "agent trace conformance requires exactly one checked run method",
                        ),
                    }
                }
            }
        }
    }

    fn trace_spec_symbol(&self, reference: &etas_hir::HirSpecRef) -> Option<etas_hir::SymbolId> {
        let etas_hir::ResolveResult::Resolved(symbol) = reference.spec_path.resolution else {
            return None;
        };
        let symbol = self.hir.canonical_source_symbol(symbol)?;
        let is_trace = self.types.facts.spec_signatures.get(&symbol).is_some_and(|signature| signature.kind == etas_types::SpecKind::TraceSpec)
            || self.hir.symbols.get(symbol).and_then(|symbol| symbol.defining_item).and_then(|item| self.hir.items.get(item)).is_some_and(|item| matches!(item, HirItem::Spec(spec) if matches!(spec.kind, etas_hir::HirSpecKind::TraceSpec)));
        is_trace.then_some(symbol)
    }

    fn require_named_conformance(
        &mut self,
        item: etas_hir::HirItemId,
        reference: &etas_hir::HirSpecRef,
    ) {
        let Some(symbol) = self.trace_spec_symbol(reference) else {
            return;
        };
        if !self.types.facts.trace_spec_conformances.iter().any(|fact| fact.item == item && fact.span == reference.span && matches!(&fact.target, etas_types::TraceSpecConformanceTarget::Named { spec_symbol, .. } if *spec_symbol == symbol)) {
            self.diagnostic(reference.span, "trace spec conformance requires checked type facts");
        }
    }
}
