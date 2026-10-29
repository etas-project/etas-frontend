use etas_code::{CallableId, descriptor::*};
use etas_effects::{CallableExitSummary, CallableOutcome, ReturnObservation};
use etas_hir::{HirItem, SymbolDef};

use super::{DefinitionDescriptorProjector, ProjectionError};

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn can_project_parametric_outcomes(
        &self,
        exits: &CallableExitSummary,
        signature: TypeInfoRef,
        parameters: &[etas_hir::SymbolId],
    ) -> bool {
        if exits.gaps.is_empty() {
            return exits.parameter_exits.is_empty();
        }
        if exits.parameter_exits.iter().any(|exit| {
            let Some(handlers) = &exit.enclosing_handlers else {
                return true;
            };
            exit.handler_free != Some(handlers.is_empty())
                || self.checked_parameter_handler_scopes(exit, exits).is_none()
        }) {
            return false;
        }
        let Some(Some(TypeInfo::Function(owner))) = self.types.get(signature.0 as usize) else {
            return false;
        };
        let coverage = etas_effects::EffectCoverage {
            registry: self.checked.effect_registry,
            types: self.checked.type_store,
        };
        if exits.gaps.iter().any(|gap| match gap {
            etas_effects::OutcomeAnalysisGap::ParameterCall { .. } => false,
            etas_effects::OutcomeAnalysisGap::Handler(source) => {
                let matching = exits
                    .handler_gaps
                    .iter()
                    .filter(|evidence| evidence.source == *source)
                    .collect::<Vec<_>>();
                matching.is_empty()
                    || matching.iter().any(|evidence| {
                        !evidence.normal_control
                            || (!exits.parameter_exits.iter().any(|exit| {
                                exit.enclosing_handlers.as_ref().is_some_and(|handlers| {
                                    handlers.iter().any(|handler| {
                                        matches!(handler, etas_effects::ParameterHandlerScopeRef::Native(handler)
                                            if self.handler_scope_source(*handler) == Some(source.span()))
                                    })
                                })
                            }) && (!exits
                                .parameter_exits
                                .iter()
                                .all(|exit| exit.handler_free == Some(true))
                                || !coverage
                                    .rows_disjoint(&evidence.handled, &evidence.body_escaping)))
                    })
            }
            _ => true,
        }) {
            return false;
        }
        if exits
            .gaps
            .iter()
            .filter(|gap| matches!(gap, etas_effects::OutcomeAnalysisGap::ParameterCall { .. }))
            .any(|gap| {
                !exits.parameter_exits.iter().any(|exit| {
                    matches!(gap,
            etas_effects::OutcomeAnalysisGap::ParameterCall { target, output, source }
                if exit.target == *target && exit.output == *output && exit.source == *source)
                })
            })
        {
            return false;
        }
        if exits.exits.is_empty() {
            return false;
        }
        if !complete_finite_prefixes(exits, self.limits) {
            return false;
        }
        exits.parameter_exits.iter().all(|exit| {
            let Some(arguments) = exit
                .arguments
                .as_deref()
                .and_then(super::analysis::checked_argument_values)
            else {
                return false;
            };
            if !exits.gaps.iter().any(|gap| {
                matches!(gap,
                etas_effects::OutcomeAnalysisGap::ParameterCall { target, output, source }
                    if exit.target == *target && exit.output == *output && exit.source == *source)
            }) {
                return false;
            }
            let Some(position) = parameters.iter().position(|symbol| {
                exit.target == etas_effects::ParameterCallTarget::Native(*symbol)
            }) else {
                return false;
            };
            let Some(parameter) = owner.parameters.get(position) else {
                return false;
            };
            let Some(Some(TypeInfo::Function(callback))) = self.types.get(parameter.ty.0 as usize)
            else {
                return false;
            };
            let Some(row) = callback
                .effects
                .and_then(|row| self.rows.get(row.0 as usize))
                .and_then(Option::as_ref)
            else {
                return false;
            };
            arguments.len() == callback.parameters.len()
                && callback.generics.is_empty()
                && row.tail.is_none()
                && row
                    .effects
                    .iter()
                    .all(|effect| matches!(effect.target, EffectTargetInfo::Action { .. }))
        })
    }

    pub(super) fn project_outcomes(
        &mut self,
        exits: &CallableExitSummary,
        callable: &CallableId,
        signature: TypeInfoRef,
        parameters: &[etas_hir::SymbolId],
        scope: usize,
    ) -> Result<TraceInfoRef, ProjectionError> {
        let count = exits
            .exits
            .len()
            .saturating_add(exits.parameter_exits.len());
        self.ensure_nodes(count.saturating_mul(2).saturating_add(1))?;
        self.ensure_edges(count.saturating_mul(3))?;
        let root = self.reserve_trace()?;
        let mut branches = Vec::with_capacity(exits.exits.len());
        for exit in &exits.exits {
            let trace = self.analysis_trace(
                &exit.trace,
                callable,
                signature,
                parameters,
                scope,
                Some(exits),
            )?;
            let outcome = match &exit.outcome {
                CallableOutcome::Returned { ty, observation } => {
                    let observation = match observation {
                        ReturnObservation::Any | ReturnObservation::CallableSources(_) => {
                            ReturnObservationInfo::Any
                        }
                        ReturnObservation::Unit => ReturnObservationInfo::Unit,
                        ReturnObservation::Bool(value) => ReturnObservationInfo::Bool(*value),
                        ReturnObservation::Variant(symbol) => {
                            let constructor = *symbol;
                            let symbol = self
                                .checked
                                .symbols
                                .get(*symbol)
                                .ok_or(ProjectionError::MissingSymbol(*symbol))?;
                            let SymbolDef::EnumVariant {
                                enum_item,
                                variant_index,
                            } = symbol.def
                            else {
                                return Err(ProjectionError::Invalid(
                                    "return variant has no checked constructor identity".into(),
                                ));
                            };
                            let Some(HirItem::Enum(decl)) = self.checked.items.get(enum_item)
                            else {
                                return Err(ProjectionError::Invalid(
                                    "return variant has no checked enum declaration".into(),
                                ));
                            };
                            let owner = self
                                .identities
                                .symbol(decl.symbol)
                                .ok_or(ProjectionError::MissingDeclaration(decl.symbol))?
                                .clone();
                            let variant =
                                decl.variants.get(variant_index as usize).ok_or_else(|| {
                                    ProjectionError::Invalid(
                                        "return constructor has no variant declaration".into(),
                                    )
                                })?;
                            if variant.symbol != constructor {
                                return Err(ProjectionError::Invalid(
                                    "return constructor differs from its declared enum slot".into(),
                                ));
                            }
                            let name = symbol.name.clone();
                            self.text(&name)?;
                            ReturnObservationInfo::Variant { owner, name }
                        }
                    };
                    CallableOutcomeInfo::Returned {
                        ty: self.type_ref(*ty, scope)?,
                        observation,
                    }
                }
                CallableOutcome::Raised { error } => CallableOutcomeInfo::Raised {
                    error: self.type_ref(*error, scope)?,
                },
                CallableOutcome::NonReturn => CallableOutcomeInfo::NonReturn,
                CallableOutcome::HandlerFinished { .. }
                | CallableOutcome::HandlerResumed { .. } => {
                    return Err(ProjectionError::Invalid(
                        "handler-local control cannot be published as a callable return".into(),
                    ));
                }
            };
            let terminal = self.reserve_trace()?;
            self.traces[terminal.0 as usize] = Some(TraceInfo::Outcome(outcome));
            let branch = self.reserve_trace()?;
            self.traces[branch.0 as usize] = Some(TraceInfo::Seq(vec![trace, terminal]));
            branches.push(branch);
        }
        for exit in &exits.parameter_exits {
            let prefix = self.analysis_trace(
                &exit.prefix,
                callable,
                signature,
                parameters,
                scope,
                Some(exits),
            )?;
            let arguments = exit
                .arguments
                .as_deref()
                .and_then(super::analysis::checked_argument_values);
            let call = super::analysis::parameter_call(
                callable,
                signature,
                parameters,
                exit.target,
                arguments,
                self.project_parameter_handler_scopes(
                    exit.target,
                    exit.source.span(),
                    Some(exits),
                    super::handler_scopes::TraceProjectionContext {
                        scope,
                        callable,
                        signature,
                        parameters,
                    },
                )?,
            )?;
            let terminal = self.reserve_trace()?;
            self.traces[terminal.0 as usize] = Some(TraceInfo::Outcome(
                CallableOutcomeInfo::ParameterNonReturn(call),
            ));
            let branch = self.reserve_trace()?;
            self.traces[branch.0 as usize] = Some(TraceInfo::Seq(vec![prefix, terminal]));
            branches.push(branch);
        }
        self.traces[root.0 as usize] = Some(TraceInfo::Choice(branches));
        Ok(root)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum FiniteTraceAtom {
    Action(etas_effects::Effect),
    Call(
        etas_effects::ParameterCallTarget,
        etas_effects::FrozenSourceLocation,
    ),
}

fn complete_finite_prefixes(exits: &CallableExitSummary, limits: DescriptorLimits) -> bool {
    use std::collections::{BTreeMap, BTreeSet};
    type Site = (
        etas_effects::ParameterCallTarget,
        etas_effects::FrozenSourceLocation,
    );
    type Prefixes = BTreeMap<Site, BTreeSet<Vec<FiniteTraceAtom>>>;
    let mut budget = limits.max_edges;
    let mut expected = Prefixes::new();
    for normal in &exits.exits {
        let Some(paths) = finite_trace_paths(&normal.trace, limits.max_nodes, &mut budget) else {
            return false;
        };
        for path in paths {
            for (index, atom) in path.iter().enumerate() {
                if let FiniteTraceAtom::Call(target, source) = atom {
                    if index > budget {
                        return false;
                    }
                    budget -= index;
                    expected
                        .entry((*target, source.clone()))
                        .or_default()
                        .insert(path[..index].to_vec());
                }
            }
        }
    }
    let mut actual = Prefixes::new();
    for exit in &exits.parameter_exits {
        let Some(paths) = finite_trace_paths(&exit.prefix, limits.max_nodes, &mut budget) else {
            return false;
        };
        actual
            .entry((exit.target, exit.source.clone()))
            .or_default()
            .extend(paths);
    }
    expected == actual
}

fn finite_trace_paths(
    trace: &etas_effects::ActionTraceDomain,
    max_paths: usize,
    budget: &mut usize,
) -> Option<std::collections::BTreeSet<Vec<FiniteTraceAtom>>> {
    use etas_effects::ActionTraceDomain as Trace;
    use std::collections::BTreeSet;
    enum Step<'a> {
        Visit(&'a Trace),
        Sequence(usize),
        Choice(usize),
    }
    let mut pending = vec![Step::Visit(trace)];
    let mut values = Vec::<BTreeSet<Vec<FiniteTraceAtom>>>::new();
    while let Some(step) = pending.pop() {
        *budget = budget.checked_sub(1)?;
        match step {
            Step::Visit(Trace::Empty) => values.push(BTreeSet::from([vec![]])),
            Step::Visit(Trace::Event(event)) => {
                values.push(BTreeSet::from([vec![FiniteTraceAtom::Action(
                    event.action.clone(),
                )]]))
            }
            Step::Visit(Trace::ParameterCall { target, span, .. }) => values.push(BTreeSet::from(
                [vec![FiniteTraceAtom::Call(*target, (*span).into())]],
            )),
            Step::Visit(Trace::Seq(parts)) => {
                pending.push(Step::Sequence(parts.len()));
                pending.extend(parts.iter().rev().map(Step::Visit));
            }
            Step::Visit(Trace::Choice(parts)) => {
                pending.push(Step::Choice(parts.len()));
                pending.extend(parts.iter().rev().map(Step::Visit));
            }
            Step::Visit(Trace::Repeat(_) | Trace::UnknownOrder(_) | Trace::Widened { .. }) => {
                return None;
            }
            Step::Sequence(length) => {
                let start = values.len().checked_sub(length)?;
                let parts = values.drain(start..).collect::<Vec<_>>();
                let mut combined = BTreeSet::from([vec![]]);
                for part in parts {
                    if combined.len().checked_mul(part.len())? > max_paths {
                        return None;
                    }
                    let mut next = BTreeSet::new();
                    for left in &combined {
                        for right in &part {
                            let size = left.len().checked_add(right.len())?;
                            *budget = budget.checked_sub(size.saturating_add(1))?;
                            let mut path = Vec::with_capacity(size);
                            path.extend_from_slice(left);
                            path.extend_from_slice(right);
                            next.insert(path);
                        }
                    }
                    combined = next;
                }
                values.push(combined);
            }
            Step::Choice(length) => {
                let start = values.len().checked_sub(length)?;
                let mut combined = BTreeSet::new();
                for part in values.drain(start..) {
                    combined.extend(part);
                    if combined.len() > max_paths {
                        return None;
                    }
                }
                values.push(combined);
            }
        }
    }
    (values.len() == 1).then(|| values.pop()).flatten()
}

#[cfg(test)]
mod tests {
    #[test]
    fn finite_trace_paths_preserve_choices_and_reject_unknown_multiplicity() {
        use etas_effects::{
            ActionTraceDomain as Trace, ParameterCallTarget, infer::domain::ParameterCallRef,
        };
        let target = ParameterCallTarget::Interface(0);
        let span = etas_core::Span::empty(etas_core::SourceId(0), etas_core::TextSize::ZERO);
        let call = Trace::ParameterCall {
            target,
            parameter: "f".into(),
            span,
        };
        let paths = |trace: &Trace| super::finite_trace_paths(trace, 100, &mut 100);
        assert_eq!(
            paths(&Trace::Choice(vec![call.clone(), call.clone()]))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            paths(&Trace::Choice(vec![Trace::Empty, call.clone()]))
                .unwrap()
                .len(),
            2
        );
        assert!(paths(&Trace::Repeat(Box::new(call.clone()))).is_none());
        assert!(
            paths(&Trace::Widened {
                actions: Default::default(),
                parameter_calls: vec![ParameterCallRef {
                    target,
                    parameter: "f".into(),
                    span
                }]
            })
            .is_none()
        );
        assert!(
            super::finite_trace_paths(&Trace::Seq(vec![call.clone(), call]), 100, &mut 1).is_none()
        );
    }

    #[test]
    fn handler_disjointness_accounts_for_effect_extensions() {
        use etas_effects::{
            DependencyEffectExtension, DependencyEffectMetadata, DependencyEffectTag,
        };

        let path = |name: &str| vec!["example".to_owned(), name.to_owned()];
        let metadata = DependencyEffectMetadata {
            source_bindings: Vec::new(),
            tags: ["Parent", "Child", "Other"]
                .into_iter()
                .map(|name| DependencyEffectTag {
                    path: path(name),
                    runtime_requirement: None,
                })
                .collect(),
            actions: Vec::new(),
            extensions: vec![DependencyEffectExtension {
                package: None,
                child: path("Child"),
                parent: path("Parent"),
            }],
        };
        let registry = etas_effects::EffectRegistry::from_hir_types_and_dependencies(
            &etas_hir::HirProgram::default(),
            &etas_types::TypeOutput::default(),
            &metadata,
        )
        .unwrap();
        let parent = registry.tag_by_name("example.Parent").unwrap();
        let child = registry.tag_by_name("example.Child").unwrap();
        let other = registry.tag_by_name("example.Other").unwrap();
        let types = etas_types::TypeStore::new();
        let coverage = etas_effects::EffectCoverage {
            registry: &registry,
            types: &types,
        };
        let row = |tag| {
            etas_effects::EffectRow::closed([etas_effects::Effect::Tag(tag)].into_iter().collect())
        };
        assert!(!coverage.rows_disjoint(&row(parent), &row(child)));
        assert!(!coverage.rows_disjoint(&row(child), &row(parent)));
        assert!(coverage.rows_disjoint(&row(parent), &row(other)));
    }
}
