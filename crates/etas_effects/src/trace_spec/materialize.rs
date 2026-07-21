use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{
    HirDeclarationConformanceTarget, HirEffectArg, HirEffectRef, HirItem, HirItemId, HirProgram,
    HirSpecExpr, HirSpecKind, ImportAliasOrigin, PartialResolutionReason, ResolveResult, SymbolDef,
    SymbolId, resolved_static_type_selector_symbol, static_selector_path_segments,
};
use etas_types::{EffectArgRef, SymbolTypeFact, TypeId};

use crate::{
    ActionInstanceRef, ActionRef, Effect, EffectActionArgKind, EffectOutput, EffectPipelineError,
    EffectRegistry, EffectRow, EffectSet, ExternalTraceSpecClauseKind,
    ExternalTraceSpecClauseMetadata, ExternalTraceSpecEffectMetadata,
    ExternalTraceSpecEffectRowMetadata, ExternalTraceSpecSummaryMetadata, RequirementFact,
    RequirementSet, TraceSpecClauseFact,
    trace_spec::TraceSpecModelStore,
    trace_spec::model::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecPattern},
};

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

struct TraceSpecMaterializer<'a> {
    hir: &'a HirProgram,
    types: &'a etas_types::TypeOutput,
    registry: &'a EffectRegistry,
    external_trace_specs: &'a [crate::AnchoredExternalMetadata<ExternalTraceSpecSummaryMetadata>],
    effects: &'a mut EffectOutput,
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
            .or_insert_with(RequirementSet::new)
            .insert(requirement.clone());
        self.effects
            .facts
            .requirements
            .symbols
            .entry(symbol)
            .or_insert_with(RequirementSet::new)
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

    fn pattern_from_effect_ref(
        &mut self,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<TraceSpecPattern>, EffectPipelineError> {
        let Some(effect) = self.effect_from_ref(effect_ref)? else {
            return Ok(None);
        };
        Ok(Some(TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(effect)),
            label: effect_label(effect_ref),
            arg_bounds: Vec::new(),
        }))
    }

    fn effect_from_ref(
        &mut self,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let resolved = match &effect_ref.path.resolution {
            ResolveResult::Resolved(symbol) => *symbol,
            ResolveResult::PartiallyResolved(partial)
                if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
            {
                if let Some(effect) = self.effect_from_partial_member_ref(effect_ref, partial)? {
                    return Ok(Some(effect));
                }
                self.diagnostic(
                    effect_ref.span,
                    "trace spec action pattern must resolve to a checked effect or action",
                );
                return Ok(None);
            }
            _ => {
                if let Some(effect) = self.effect_from_standard_path(effect_ref)? {
                    return Ok(Some(effect));
                }
                self.diagnostic(
                    effect_ref.span,
                    "trace spec action pattern must resolve to a checked effect or action",
                );
                return Ok(None);
            }
        };
        if let Some(signature) = self.registry.action(resolved) {
            let action = ActionRef {
                tag: signature.owner,
                action: signature.id,
            };
            let Some(args) = self.action_args(effect_ref, signature)? else {
                return Ok(None);
            };
            return if args.is_empty() {
                Ok(Some(Effect::Action(action)))
            } else {
                Ok(Some(Effect::AppliedAction(ActionInstanceRef {
                    action,
                    args,
                })))
            };
        }
        if let Some(tag) = self.registry.tag_by_symbol(resolved) {
            let Some(args) = self.effect_type_args(effect_ref) else {
                return Ok(None);
            };
            return if args.is_empty() {
                Ok(Some(Effect::Tag(tag)))
            } else {
                Ok(Some(Effect::Applied { tag, args }))
            };
        }
        if let Some(effect) = self.effect_from_import_alias(resolved, effect_ref)? {
            return Ok(Some(effect));
        }
        self.diagnostic(
            effect_ref.span,
            "trace spec action pattern must resolve to an effect tag or action",
        );
        Ok(None)
    }

    fn effect_from_standard_path(
        &mut self,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let qualified = effect_label(effect_ref);
        if let Some(action) = self.registry.standard_action_by_name(&qualified) {
            let Some(signature) = self.registry.action_signature(&action) else {
                self.diagnostic(
                    effect_ref.span,
                    "trace spec action is missing registry signature",
                );
                return Ok(None);
            };
            let Some(args) = self.action_args(effect_ref, signature)? else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Action(action)
            } else {
                Effect::AppliedAction(ActionInstanceRef { action, args })
            }));
        }
        if let Some(tag) = self.registry.standard_tag_by_name(&qualified) {
            let Some(args) = self.effect_type_args(effect_ref) else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Tag(tag)
            } else {
                Effect::Applied { tag, args }
            }));
        }
        Ok(None)
    }

    fn effect_from_partial_member_ref(
        &mut self,
        effect_ref: &HirEffectRef,
        partial: &etas_hir::PartialResolution,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let Some(prefix) = partial.resolved_prefix else {
            return Ok(None);
        };
        let Some(tag) = self.registry.tag_by_symbol(prefix) else {
            return Ok(None);
        };
        if partial.remaining.len() != 1 {
            return Ok(None);
        }
        let action_name = format!(
            "{}.{}",
            effect_ref
                .path
                .segments
                .iter()
                .take(effect_ref.path.segments.len().saturating_sub(1))
                .map(|segment| segment.name.as_str())
                .collect::<Vec<_>>()
                .join("."),
            partial.remaining[0]
        );
        let Some(action) = self.registry.action_by_name(&action_name) else {
            return Ok(None);
        };
        if action.tag != tag {
            return Ok(None);
        }
        let Some(signature) = self.registry.action_signature(&action) else {
            self.diagnostic(
                effect_ref.span,
                "trace spec action is missing registry signature",
            );
            return Ok(None);
        };
        let Some(args) = self.action_args(effect_ref, signature)? else {
            return Ok(None);
        };
        Ok(Some(if args.is_empty() {
            Effect::Action(action)
        } else {
            Effect::AppliedAction(ActionInstanceRef { action, args })
        }))
    }

    fn effect_from_import_alias(
        &mut self,
        symbol: SymbolId,
        effect_ref: &HirEffectRef,
    ) -> Result<Option<Effect>, EffectPipelineError> {
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return Ok(None);
        };
        let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
            return Ok(None);
        };
        if !matches!(
            origin,
            ImportAliasOrigin::StdPrelude | ImportAliasOrigin::SourceImport
        ) {
            return Ok(None);
        }
        let qualified = path.join(".");
        if let Some(action) = self.registry.action_by_name(&qualified) {
            let Some(signature) = self.registry.action_signature(&action) else {
                self.diagnostic(
                    effect_ref.span,
                    "trace spec action is missing registry signature",
                );
                return Ok(None);
            };
            let Some(args) = self.action_args(effect_ref, signature)? else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Action(action)
            } else {
                Effect::AppliedAction(ActionInstanceRef { action, args })
            }));
        }
        if let Some(tag) = self.registry.tag_by_name(&qualified) {
            let Some(args) = self.effect_type_args(effect_ref) else {
                return Ok(None);
            };
            return Ok(Some(if args.is_empty() {
                Effect::Tag(tag)
            } else {
                Effect::Applied { tag, args }
            }));
        }
        Ok(None)
    }

    fn action_args(
        &mut self,
        effect_ref: &HirEffectRef,
        signature: &crate::EffectActionSig,
    ) -> Result<Option<Vec<EffectArgRef>>, EffectPipelineError> {
        if !effect_ref.args.is_empty() && effect_ref.args.len() != signature.effect_args.len() {
            self.diagnostic(
                effect_ref.span,
                format!(
                    "trace spec action pattern expects {} static selector argument(s), got {}",
                    signature.effect_args.len(),
                    effect_ref.args.len()
                ),
            );
            return Ok(None);
        }
        if effect_ref.args.is_empty() {
            return Ok(Some(
                signature
                    .effect_args
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        signature
                            .selector_defaults
                            .get(index)
                            .and_then(Clone::clone)
                            .unwrap_or(EffectArgRef::Wildcard)
                    })
                    .collect(),
            ));
        }
        let args = signature
            .effect_args
            .iter()
            .zip(&effect_ref.args)
            .map(|(kind, arg)| self.effect_arg(arg, kind))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(args.into_iter().collect())
    }

    fn effect_type_args(&mut self, effect_ref: &HirEffectRef) -> Option<Vec<TypeId>> {
        effect_ref
            .args
            .iter()
            .map(|arg| match arg {
                HirEffectArg::Type(ty) => {
                    self.types.facts.type_refs.get(ty).copied().or_else(|| {
                        self.diagnostic(
                            effect_ref.span,
                            "trace spec type argument is missing checked type facts",
                        );
                        None
                    })
                }
                _ => {
                    self.diagnostic(
                        effect_ref.span,
                        "effect tag trace spec arguments must be types",
                    );
                    None
                }
            })
            .collect()
    }

    fn effect_arg(
        &mut self,
        arg: &HirEffectArg,
        kind: &EffectActionArgKind,
    ) -> Result<Option<EffectArgRef>, EffectPipelineError> {
        match arg {
            HirEffectArg::Wildcard { .. } => Ok(Some(EffectArgRef::Wildcard)),
            HirEffectArg::String { value, .. } => Ok(Some(EffectArgRef::String(value.clone()))),
            HirEffectArg::Int { text, .. } => Ok(Some(EffectArgRef::Int(text.clone()))),
            HirEffectArg::Type(ty) => match self.types.facts.type_refs.get(ty).copied() {
                Some(type_id) => Ok(Some(EffectArgRef::Type(type_id))),
                None => {
                    let span = effect_ref_span_for_missing_type(self.hir, *ty)?;
                    self.diagnostic(
                        span,
                        "trace spec selector type argument is missing checked type facts",
                    );
                    Ok(None)
                }
            },
            HirEffectArg::Path(path) => match kind {
                EffectActionArgKind::Type => Ok(self
                    .type_arg_from_path(path)
                    .map(EffectArgRef::Type)
                    .or_else(|| {
                        self.diagnostic(path.span, "trace spec selector must name a checked type");
                        None
                    })),
                EffectActionArgKind::MemoryPlace
                | EffectActionArgKind::StaticResourcePath { .. }
                | EffectActionArgKind::StringPattern => Ok(self
                    .static_path_segments(path)
                    .map(EffectArgRef::Path)
                    .or_else(|| {
                        self.diagnostic(
                            path.span,
                            "trace spec selector path must resolve to a static resource",
                        );
                        None
                    })),
            },
        }
    }

    fn type_arg_from_path(&self, path: &etas_hir::ResolvedPath) -> Option<TypeId> {
        let symbol = resolved_static_type_selector_symbol(path, Some)?;
        if let Some(symbol_data) = self.hir.symbols.get(symbol)
            && matches!(symbol_data.def, etas_hir::SymbolDef::TypeParam { .. })
        {
            return self.types.facts.symbol_types.get(&symbol).and_then(|fact| {
                if let SymbolTypeFact::Param { ty } = fact {
                    Some(*ty)
                } else {
                    None
                }
            });
        }
        match self.types.facts.symbol_types.get(&symbol) {
            Some(SymbolTypeFact::TypeAlias { target, .. }) => Some(*target),
            Some(SymbolTypeFact::Type { constructor })
            | Some(SymbolTypeFact::NominalType { constructor, .. }) => Some(TypeId(constructor.0)),
            _ => None,
        }
    }

    fn static_path_segments(&self, path: &etas_hir::ResolvedPath) -> Option<Vec<String>> {
        static_selector_path_segments(self.hir, path, Some)
    }

    fn trace_spec_clauses_for_ref(
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
        let Some(clauses) = self.clauses_from_external_summary(summary, spec_ref.span) else {
            return None;
        };
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
        let Some(item) = symbol.defining_item else {
            return None;
        };
        match self.hir.items.get(item) {
            Some(HirItem::Spec(spec)) if matches!(spec.kind, HirSpecKind::TraceSpec) => Some(item),
            _ => None,
        }
    }

    fn clauses_from_external_summary(
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

    fn symbol_name(&self, symbol: SymbolId) -> String {
        self.hir
            .symbols
            .get(symbol)
            .map(|symbol| symbol.name.clone())
            .unwrap_or_else(|| "unknown".to_owned())
    }

    fn diagnostic(&mut self, span: Span, message: impl Into<String>) {
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
