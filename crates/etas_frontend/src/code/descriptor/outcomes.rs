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
            if exact_site_count(&exit.prefix, exit.target, exit.source.span()) != Some(0)
                || exits.exits.iter().any(|normal| {
                    exact_site_count(&normal.trace, exit.target, exit.source.span()) != Some(1)
                })
            {
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

fn exact_site_count(
    trace: &etas_effects::ActionTraceDomain,
    target: etas_effects::ParameterCallTarget,
    source: etas_core::Span,
) -> Option<usize> {
    use etas_effects::ActionTraceDomain;
    enum Step<'a> {
        Visit(&'a ActionTraceDomain),
        Sequence(usize),
        Choice(usize),
        Repeat,
    }

    let mut pending = vec![Step::Visit(trace)];
    let mut counts: Vec<Option<usize>> = Vec::new();
    while let Some(step) = pending.pop() {
        match step {
            Step::Visit(
                ActionTraceDomain::Empty
                | ActionTraceDomain::Event(_)
                | ActionTraceDomain::UnknownOrder(_),
            ) => counts.push(Some(0)),
            Step::Visit(ActionTraceDomain::ParameterCall {
                target: actual,
                span,
                ..
            }) => {
                counts.push(Some(usize::from(*actual == target && *span == source)));
            }
            Step::Visit(ActionTraceDomain::Seq(parts)) => {
                pending.push(Step::Sequence(parts.len()));
                pending.extend(parts.iter().map(Step::Visit));
            }
            Step::Visit(ActionTraceDomain::Choice(parts)) => {
                pending.push(Step::Choice(parts.len()));
                pending.extend(parts.iter().map(Step::Visit));
            }
            Step::Visit(ActionTraceDomain::Repeat(inner)) => {
                pending.push(Step::Repeat);
                pending.push(Step::Visit(inner));
            }
            Step::Visit(ActionTraceDomain::Widened {
                parameter_calls, ..
            }) => {
                counts.push(
                    (!parameter_calls
                        .iter()
                        .any(|call| call.target == target && call.span == source))
                    .then_some(0),
                );
            }
            Step::Sequence(length) => {
                let start = counts.len().checked_sub(length)?;
                let count = counts
                    .drain(start..)
                    .try_fold(0usize, |sum, next| sum.checked_add(next?));
                counts.push(count);
            }
            Step::Choice(length) => {
                let start = counts.len().checked_sub(length)?;
                let mut alternatives = counts.drain(start..);
                let first = alternatives.next().flatten();
                let count = first
                    .filter(|first| alternatives.all(|alternative| alternative == Some(*first)));
                drop(alternatives);
                counts.push(count);
            }
            Step::Repeat => {
                let count = counts.pop()?;
                counts.push((count == Some(0)).then_some(0));
            }
        }
    }
    if counts.len() == 1 {
        counts.pop()?
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parameter_site_count_handles_choices_and_preserves_unknown_multiplicity() {
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
        let count = |trace: &Trace| super::exact_site_count(trace, target, span);
        assert_eq!(
            count(&Trace::Choice(vec![call.clone(), call.clone()])),
            Some(1)
        );
        assert_eq!(
            count(&Trace::Seq(vec![
                Trace::Choice(vec![Trace::Empty, Trace::Empty]),
                call.clone()
            ])),
            Some(1)
        );
        assert_eq!(
            count(&Trace::Choice(vec![Trace::Empty, call.clone()])),
            None
        );
        assert_eq!(count(&Trace::Repeat(Box::new(call.clone()))), None);
        assert_eq!(count(&Trace::Repeat(Box::new(Trace::Empty))), Some(0));
        assert_eq!(
            count(&Trace::Widened {
                actions: Default::default(),
                parameter_calls: vec![ParameterCallRef {
                    target,
                    parameter: "f".into(),
                    span
                }]
            }),
            None
        );
        assert_eq!(
            count(&Trace::Widened {
                actions: Default::default(),
                parameter_calls: vec![]
            }),
            Some(0)
        );
        assert_eq!(count(&Trace::Choice(vec![])), None);
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
