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
        // A callable's escaping row does not bound its default-handled
        // requests. Until handler-aware callback outcomes are available, an
        // enclosing handler can change its control flow even for ![].
        if exits
            .parameter_exits
            .iter()
            .any(|exit| exit.handler_free != Some(true))
        {
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
                            || !coverage.rows_disjoint(&evidence.handled, &evidence.body_escaping)
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
            if linear_site_count(&exit.prefix, exit.target, exit.source.span()) != Some(0)
                || exits.exits.iter().any(|normal| {
                    linear_site_count(&normal.trace, exit.target, exit.source.span()) != Some(1)
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
                && (row.effects.is_empty()
                    || ((exit.handler_free == Some(true)
                        || exit
                            .enclosing_handled
                            .as_ref()
                            .is_some_and(|handled| self.handler_row_disjoint(handled, row)))
                        && row.effects.iter().all(|effect| {
                            matches!(effect.target, EffectTargetInfo::Action { .. })
                        })))
        })
    }

    fn handler_row_disjoint(
        &self,
        handled: &etas_effects::EffectRow,
        callback: &EffectRowInfo,
    ) -> bool {
        if handled.open.is_some() {
            return false;
        }
        handled.effects.iter().all(|effect| {
            let (tag, action) = match effect {
                etas_effects::Effect::Tag(tag) | etas_effects::Effect::Applied { tag, .. } => {
                    (*tag, None)
                }
                etas_effects::Effect::Action(action) => (action.tag, Some(action)),
                etas_effects::Effect::AppliedAction(instance) => {
                    (instance.action.tag, Some(&instance.action))
                }
                etas_effects::Effect::Error(_) => {
                    let Some(tag) = self
                        .checked
                        .effect_registry
                        .tag_by_core(etas_effects::CoreEffect::Error)
                    else {
                        return false;
                    };
                    (tag, None)
                }
                etas_effects::Effect::Var(_) => return false,
            };
            if self.identities.effect_tag(tag).is_err() {
                return false;
            }
            let action_name = match action {
                Some(action) => match self.checked.effect_registry.action_signature(action) {
                    Some(signature) => Some(signature.name.as_str()),
                    None => return false,
                },
                None => None,
            };
            callback.effects.iter().all(|effect| match &effect.target {
                EffectTargetInfo::Action { effect, name } => {
                    let mut candidates = self.checked.effect_registry.tags().filter_map(|tag| {
                        self.identities
                            .effect_tag(tag.id)
                            .ok()
                            .filter(|identity| *identity == effect)
                            .map(|_| tag.id)
                    });
                    let Some(callback_tag) = candidates.next() else {
                        return false;
                    };
                    if candidates.next().is_some() {
                        return false;
                    }
                    action_tags_disjoint(
                        self.checked.effect_registry,
                        tag,
                        action_name,
                        callback_tag,
                        name,
                    )
                }
                EffectTargetInfo::Effect(_) => false,
            })
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

fn action_tags_disjoint(
    registry: &etas_effects::EffectRegistry,
    handled: etas_effects::EffectTagId,
    handled_action: Option<&str>,
    callback: etas_effects::EffectTagId,
    callback_action: &str,
) -> bool {
    if registry.tag_name(handled).is_none()
        || registry.tag_name(callback).is_none()
        || !registry.unresolved_dependency_extensions().is_empty()
    {
        return false;
    }
    if handled == callback {
        return handled_action.is_some_and(|action| action != callback_action);
    }
    !registry.tag_extends(handled, callback) && !registry.tag_extends(callback, handled)
}

fn linear_site_count(
    trace: &etas_effects::ActionTraceDomain,
    target: etas_effects::ParameterCallTarget,
    source: etas_core::Span,
) -> Option<usize> {
    use etas_effects::ActionTraceDomain;
    let mut pending = vec![trace];
    let mut count = 0usize;
    while let Some(node) = pending.pop() {
        match node {
            ActionTraceDomain::Empty | ActionTraceDomain::Event(_) => {}
            ActionTraceDomain::ParameterCall {
                target: actual,
                span,
                ..
            } => {
                if *actual == target && *span == source {
                    count = count.checked_add(1)?;
                }
            }
            ActionTraceDomain::Seq(parts) => pending.extend(parts),
            ActionTraceDomain::Choice(_)
            | ActionTraceDomain::Repeat(_)
            | ActionTraceDomain::UnknownOrder(_)
            | ActionTraceDomain::Widened { .. } => return None,
        }
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_disjointness_accounts_for_effect_extensions() {
        use etas_effects::{
            DependencyEffectExtension, DependencyEffectMetadata, DependencyEffectTag,
        };

        let path = |name: &str| vec!["example".to_owned(), name.to_owned()];
        let metadata = DependencyEffectMetadata {
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
        assert!(!action_tags_disjoint(&registry, parent, None, child, "act"));
        assert!(!action_tags_disjoint(
            &registry,
            child,
            Some("a"),
            parent,
            "b"
        ));
        assert!(action_tags_disjoint(&registry, parent, None, other, "act"));
        assert!(action_tags_disjoint(
            &registry,
            parent,
            Some("a"),
            parent,
            "b"
        ));
        assert!(!action_tags_disjoint(
            &registry,
            parent,
            Some("a"),
            parent,
            "a"
        ));
    }
}
