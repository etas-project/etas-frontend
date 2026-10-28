use etas_code::{CallableId, DefinitionKey, descriptor::*};
use etas_effects::trace_spec::{TraceSpecArgBoundTarget, TraceSpecClause, TraceSpecPattern};
use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_types::{
    CallableSpecSatisfactionFact, TraceSpecConformanceFact, TraceSpecConformanceTarget, TypeFacts,
    TypeId, TypeSpecSatisfactionFact,
};

use super::{DefinitionDescriptorProjector, ProjectionError};

enum PendingSpec<'a> {
    Named(SymbolId, &'a [TypeId]),
    Inline(&'a [Vec<TraceSpecClause>]),
    Protocol(DefinitionKey, Vec<TypeId>),
}

/// Built once for a catalog, so projecting many agents does not repeatedly scan
/// every conformance in the project.
pub(crate) struct AttachedSpecIndex<'a> {
    components: HashMap<TypeId, Vec<&'a TypeSpecSatisfactionFact>>,
    callables: HashMap<HirItemId, Vec<&'a CallableSpecSatisfactionFact>>,
    traces: HashMap<HirItemId, Vec<&'a TraceSpecConformanceFact>>,
}

impl<'a> AttachedSpecIndex<'a> {
    pub(crate) fn new(facts: &'a TypeFacts) -> Self {
        let mut index = Self {
            components: HashMap::new(),
            callables: HashMap::new(),
            traces: HashMap::new(),
        };
        for fact in &facts.type_spec_satisfactions {
            index
                .components
                .entry(fact.self_type)
                .or_default()
                .push(fact);
        }
        for fact in &facts.callable_spec_satisfactions {
            index.callables.entry(fact.item).or_default().push(fact);
        }
        for fact in &facts.trace_spec_conformances {
            index.traces.entry(fact.item).or_default().push(fact);
        }
        index
    }
}

impl DefinitionDescriptorProjector<'_> {
    pub(crate) fn attached_specs(
        &mut self,
        index: &AttachedSpecIndex<'_>,
        item: HirItemId,
        component: TypeId,
        identity: &DefinitionKey,
        default_entry: Option<&CallableId>,
    ) -> Result<Vec<AttachedSpecInfo>, ProjectionError> {
        let checked = self.checked;
        let default_body = checked
            .types
            .agent_default_method(item)
            .map(|method| method.body);
        let owners = [Some(item), default_body.filter(|body| *body != item)];
        let mut pending = Vec::new();
        for fact in index.components.get(&component).into_iter().flatten() {
            pending.push((fact.span, PendingSpec::Named(fact.spec_symbol, &fact.args)));
        }
        for fact in owners
            .into_iter()
            .flatten()
            .flat_map(|owner| index.callables.get(&owner).into_iter().flatten())
        {
            pending.push((fact.span, PendingSpec::Named(fact.spec_symbol, &fact.args)));
        }
        for fact in owners
            .into_iter()
            .flatten()
            .flat_map(|owner| index.traces.get(&owner).into_iter().flatten())
        {
            let spec = match &fact.target {
                TraceSpecConformanceTarget::Named { spec_symbol, args } => {
                    PendingSpec::Named(*spec_symbol, args)
                }
                TraceSpecConformanceTarget::Inline => {
                    let models = checked
                        .effects
                        .and_then(|effects| effects.trace_specs.conformances.get(&fact.item));
                    let mut matches = models
                        .into_iter()
                        .flatten()
                        .filter(|model| model.span == fact.span);
                    let model = match (matches.next(), matches.next()) {
                        (Some(model), None) => model,
                        _ => {
                            return Err(ProjectionError::Invalid(
                                "inline conformance requires exactly one checked trace model"
                                    .into(),
                            ));
                        }
                    };
                    PendingSpec::Inline(&model.alternatives)
                }
            };
            pending.push((fact.span, spec));
        }
        if !matches!(checked.items.get(item), Some(HirItem::Agent(_))) {
            return Err(ProjectionError::Invalid(
                "agent spec projection requires its declaration".into(),
            ));
        }
        for owner in owners.into_iter().flatten() {
            let conformances = match checked.items.get(owner) {
                Some(HirItem::Agent(agent)) => &agent.conformances,
                Some(HirItem::Flow(flow)) => &flow.conformances,
                Some(HirItem::Tool(tool)) => &tool.conformances,
                _ => {
                    return Err(ProjectionError::Invalid(
                        "agent conformance owner has no callable declaration".into(),
                    ));
                }
            };
            for conformance in conformances {
                let span = match &conformance.target {
                    etas_hir::HirDeclarationConformanceTarget::Path(reference) => reference.span,
                    etas_hir::HirDeclarationConformanceTarget::InlineTraceSpec(expression) => {
                        expression.span()
                    }
                    etas_hir::HirDeclarationConformanceTarget::Error { span } => *span,
                };
                if pending.iter().any(|(fact_span, _)| *fact_span == span) {
                    continue;
                }
                if let etas_hir::HirDeclarationConformanceTarget::Path(reference) =
                    &conformance.target
                    && let etas_hir::ResolveResult::Resolved(symbol) =
                        reference.spec_path.resolution
                    && let Some(identity) = self.identities.symbol(symbol)
                    && identity.kind() == etas_code::DefinitionKind::Protocol
                {
                    let arguments = reference
                        .spec_args
                        .iter()
                        .map(|ty| {
                            checked
                                .types
                                .type_refs
                                .get(ty)
                                .copied()
                                .ok_or(ProjectionError::MissingTypeReference(*ty))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    pending.push((span, PendingSpec::Protocol(identity.clone(), arguments)));
                } else {
                    return Err(ProjectionError::Invalid(
                        "agent declaration conformance has no checked spec fact".into(),
                    ));
                }
            }
        }
        pending.sort_by_key(|(span, _)| (span.source.0, span.range.start, span.range.end));
        self.ensure_edges(pending.len())?;
        let owner = match default_entry {
            Some(entry) => match (entry.as_declaration(), entry.as_method()) {
                (Some(owner), None) => GenericOwner::Declaration(owner.clone()),
                (None, Some(method)) => GenericOwner::Method(method.clone()),
                _ => {
                    return Err(ProjectionError::Invalid(
                        "invalid default callable identity".into(),
                    ));
                }
            },
            None => GenericOwner::Declaration(identity.clone()),
        };
        let scope = match self.scopes.iter().position(|scope| scope.owner == owner) {
            Some(scope) => scope,
            None => self.scope(owner, &[])?,
        };
        let mut output = Vec::with_capacity(pending.len());
        for (_, spec) in pending {
            self.edge()?;
            output.push(match spec {
                PendingSpec::Named(symbol, args) => {
                    AttachedSpecInfo::Named(self.spec_info(symbol, args, scope)?)
                }
                PendingSpec::Protocol(definition, arguments) => {
                    self.ensure_edges(arguments.len())?;
                    let arguments = arguments
                        .iter()
                        .map(|ty| self.type_ref(*ty, scope))
                        .collect::<Result<_, _>>()?;
                    AttachedSpecInfo::Protocol {
                        definition,
                        arguments,
                    }
                }
                PendingSpec::Inline(alternatives) => {
                    self.ensure_edges(alternatives.len())?;
                    let mut projected = Vec::with_capacity(alternatives.len());
                    for alternative in alternatives {
                        self.edge()?;
                        self.ensure_edges(alternative.len())?;
                        let mut rules = Vec::with_capacity(alternative.len());
                        for clause in alternative {
                            self.edge()?;
                            rules.push(self.trace_rule(clause, scope)?);
                        }
                        projected.push(rules);
                    }
                    AttachedSpecInfo::InlineTrace(std::sync::Arc::new(TraceConstraintInfo {
                        alternatives: projected,
                    }))
                }
            });
        }
        Ok(output)
    }

    fn spec_info(
        &mut self,
        symbol: SymbolId,
        args: &[TypeId],
        scope: usize,
    ) -> Result<SpecInfo, ProjectionError> {
        let definition = self
            .identities
            .symbol(symbol)
            .ok_or(ProjectionError::MissingDeclaration(symbol))?
            .clone();
        if definition.kind() != etas_code::DefinitionKind::Spec {
            return Err(ProjectionError::Invalid(
                "checked spec fact has a non-spec declaration identity".into(),
            ));
        }
        self.ensure_edges(args.len())?;
        let arguments = args
            .iter()
            .map(|ty| self.type_ref(*ty, scope))
            .collect::<Result<_, _>>()?;
        Ok(SpecInfo {
            definition,
            arguments,
        })
    }

    fn trace_rule(
        &mut self,
        clause: &TraceSpecClause,
        scope: usize,
    ) -> Result<TraceRuleInfo, ProjectionError> {
        Ok(match clause {
            TraceSpecClause::Allow { pattern, .. } => {
                TraceRuleInfo::Allow(self.trace_pattern(pattern, scope)?)
            }
            TraceSpecClause::Deny { pattern, .. } => {
                TraceRuleInfo::Deny(self.trace_pattern(pattern, scope)?)
            }
            TraceSpecClause::RequireBefore { guard, target, .. } => TraceRuleInfo::RequireBefore {
                guard: self.trace_pattern(guard, scope)?,
                target: self.trace_pattern(target, scope)?,
            },
            TraceSpecClause::RequireAfter {
                target, obligation, ..
            } => TraceRuleInfo::RequireAfter {
                target: self.trace_pattern(target, scope)?,
                obligation: self.trace_pattern(obligation, scope)?,
            },
            TraceSpecClause::Limit {
                fact: etas_effects::TraceSpecClauseFact::Limit { count },
            } => TraceRuleInfo::Limit { count: *count },
            TraceSpecClause::Limit { .. } => {
                return Err(ProjectionError::Invalid(
                    "typed trace limit has an inconsistent clause fact".into(),
                ));
            }
        })
    }

    fn trace_pattern(
        &mut self,
        pattern: &TraceSpecPattern,
        scope: usize,
    ) -> Result<TracePatternInfo, ProjectionError> {
        let row = self.analysis_row(&pattern.row, scope)?;
        self.ensure_edges(pattern.arg_bounds.len())?;
        let mut bounds = Vec::with_capacity(pattern.arg_bounds.len());
        for bound in &pattern.arg_bounds {
            self.edge()?;
            let target = match &bound.target {
                TraceSpecArgBoundTarget::Action(action) => {
                    self.analysis_effect(&etas_effects::Effect::Action(action.clone()), scope)?
                        .target
                }
                TraceSpecArgBoundTarget::EffectTag(tag) => {
                    self.analysis_effect(&etas_effects::Effect::Tag(*tag), scope)?
                        .target
                }
            };
            bounds.push(TraceSelectorBoundInfo {
                target,
                argument: u32::try_from(bound.arg_index).map_err(|_| {
                    ProjectionError::Invalid("trace argument index exceeds descriptor ABI".into())
                })?,
                spec: self.spec_info(bound.spec_symbol, &bound.spec_args, scope)?,
            });
        }
        Ok(TracePatternInfo { row, bounds })
    }
}
use std::collections::HashMap;
