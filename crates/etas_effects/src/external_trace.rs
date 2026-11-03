//! Checked, bounded lowering shared by external call inference and trace validation.

use std::collections::BTreeMap;

use etas_core::Span;

use crate::{
    ActionEvent, ActionTraceDomain, Effect, EffectRegistry, EffectSet, ExternalActionTraceMetadata,
    ExternalEffectMetadata, ParameterCallTarget, infer::domain::ParameterCallRef,
};

pub(crate) fn lower_external_action(
    registry: &EffectRegistry,
    types: &etas_types::TypeStore,
    metadata: &ExternalEffectMetadata,
) -> Option<Effect> {
    let action = registry.action_by_name(&metadata.path.join("."))?;
    let signature = registry.action_signature(&action)?;
    let args = if metadata.args.is_empty() && !signature.effect_args.is_empty() {
        signature
            .effect_args
            .iter()
            .enumerate()
            .map(|(index, _)| {
                signature
                    .selector_defaults
                    .get(index)
                    .and_then(Option::as_ref)
                    .cloned()
                    .unwrap_or(etas_types::EffectArgRef::Wildcard)
            })
            .collect()
    } else {
        metadata.args.clone()
    };
    crate::validate::checked_external_action_effect(registry, types, action, &args)
}

pub(crate) fn parameter_target(
    parameters: &[(String, ParameterCallTarget)],
    label: &str,
) -> Option<ParameterCallTarget> {
    let mut matches = parameters.iter().filter(|(name, _)| name == label);
    match (matches.next(), matches.next()) {
        (Some((_, target)), None) => Some(*target),
        _ => None,
    }
}

pub(crate) fn lower_external_action_trace(
    trace: &ExternalActionTraceMetadata,
    parameters: &[(String, ParameterCallTarget)],
    span: Span,
    resolve: impl Fn(&ExternalEffectMetadata) -> Option<Effect>,
) -> Option<ActionTraceDomain> {
    if parameters.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
        return None;
    }
    let mut index = BTreeMap::new();
    for (name, target) in parameters {
        if index.insert(name.as_str(), *target).is_some() {
            return None;
        }
    }
    let mut remaining = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    lower(
        trace,
        span,
        &resolve,
        &|target| match target {
            crate::ExternalCallableRequestTarget::Parameter(name) => {
                index.get(name.as_str()).copied()
            }
            crate::ExternalCallableRequestTarget::SpecMethod(_) => None,
        },
        0,
        &mut remaining,
    )
}

pub(crate) fn lower_external_action_trace_with_targets(
    trace: &ExternalActionTraceMetadata,
    span: Span,
    resolve: impl Fn(&ExternalEffectMetadata) -> Option<Effect>,
    target: impl Fn(&crate::ExternalCallableRequestTarget) -> Option<ParameterCallTarget>,
) -> Option<ActionTraceDomain> {
    let mut remaining = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    lower(trace, span, &resolve, &target, 0, &mut remaining)
}

fn request_label(target: &crate::ExternalCallableRequestTarget) -> &str {
    match target {
        crate::ExternalCallableRequestTarget::Parameter(name) => name,
        crate::ExternalCallableRequestTarget::SpecMethod(method) => &method.method,
    }
}

fn charge(remaining: &mut usize, amount: usize) -> Option<()> {
    *remaining = remaining.checked_sub(amount)?;
    Some(())
}

fn lower(
    trace: &ExternalActionTraceMetadata,
    span: Span,
    resolve: &impl Fn(&ExternalEffectMetadata) -> Option<Effect>,
    target: &impl Fn(&crate::ExternalCallableRequestTarget) -> Option<ParameterCallTarget>,
    depth: usize,
    remaining: &mut usize,
) -> Option<ActionTraceDomain> {
    if depth > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH {
        return None;
    }
    charge(remaining, 1)?;
    let checked_action = |action: &ExternalEffectMetadata| {
        let effect = resolve(action)?;
        effect.is_action().then_some(effect)
    };
    Some(match trace {
        ExternalActionTraceMetadata::Empty => ActionTraceDomain::Empty,
        ExternalActionTraceMetadata::Event { action, source } => {
            ActionTraceDomain::Event(ActionEvent {
                action: checked_action(action)?,
                span,
                source: source.clone(),
            })
        }
        ExternalActionTraceMetadata::ParameterCall { parameter } => {
            ActionTraceDomain::ParameterCall {
                target: target(parameter)?,
                parameter: request_label(parameter).to_owned(),
                span,
            }
        }
        ExternalActionTraceMetadata::Seq(children)
        | ExternalActionTraceMetadata::Choice(children) => {
            if matches!(trace, ExternalActionTraceMetadata::Choice(_)) && children.is_empty() {
                return None;
            }
            if children.len() > *remaining {
                return None;
            }
            let children = children
                .iter()
                .map(|child| lower(child, span, resolve, target, depth + 1, remaining))
                .collect::<Option<Vec<_>>>()?;
            if matches!(trace, ExternalActionTraceMetadata::Seq(_)) {
                ActionTraceDomain::Seq(children)
            } else {
                ActionTraceDomain::Choice(children)
            }
        }
        ExternalActionTraceMetadata::Repeat(child) => ActionTraceDomain::Repeat(Box::new(lower(
            child,
            span,
            resolve,
            target,
            depth + 1,
            remaining,
        )?)),
        ExternalActionTraceMetadata::UnknownOrder(actions) => {
            charge(remaining, actions.len())?;
            ActionTraceDomain::UnknownOrder(EffectSet::from_iter(
                actions
                    .iter()
                    .map(checked_action)
                    .collect::<Option<Vec<_>>>()?,
            ))
        }
        ExternalActionTraceMetadata::Widened {
            actions,
            parameter_calls,
        } => {
            charge(remaining, actions.len().checked_add(parameter_calls.len())?)?;
            ActionTraceDomain::Widened {
                actions: EffectSet::from_iter(
                    actions
                        .iter()
                        .map(checked_action)
                        .collect::<Option<Vec<_>>>()?,
                ),
                parameter_calls: parameter_calls
                    .iter()
                    .map(|parameter| {
                        Some(ParameterCallRef {
                            target: target(parameter)?,
                            parameter: request_label(parameter).to_owned(),
                            span,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_core::{SourceId, TextRange, TextSize};

    #[test]
    fn external_action_selectors_use_checked_arity_and_kind() {
        use crate::{
            DependencyEffectAction, DependencyEffectMetadata, DependencyEffectTag,
            EffectActionArgKind,
        };
        use etas_types::EffectArgRef;
        let types = etas_types::TypeOutput::default();
        let metadata = DependencyEffectMetadata {
            tags: vec![DependencyEffectTag {
                path: vec!["dep".into(), "Gate".into()],
                runtime_requirement: None,
            }],
            actions: vec![
                DependencyEffectAction {
                    path: vec!["dep".into(), "Gate".into(), "typed".into()],
                    effect_args: vec![EffectActionArgKind::Type],
                    selector_param_names: vec!["T".into()],
                    selector_defaults: vec![None],
                    returns_never: false,
                    runtime_requirement: None,
                },
                DependencyEffectAction {
                    path: vec!["dep".into(), "Gate".into(), "resource".into()],
                    effect_args: vec![EffectActionArgKind::StaticResourcePath {
                        ty: "Path".into(),
                    }],
                    selector_param_names: vec!["R".into()],
                    selector_defaults: vec![None],
                    returns_never: false,
                    runtime_requirement: None,
                },
            ],
            ..Default::default()
        };
        let registry = EffectRegistry::from_hir_types_and_dependencies(
            &etas_hir::HirProgram::default(),
            &types,
            &metadata,
        )
        .unwrap();
        let action = |name: &str, args| ExternalEffectMetadata {
            path: vec!["dep".into(), "Gate".into(), name.into()],
            args,
        };
        assert!(
            lower_external_action(
                &registry,
                &types.store,
                &action("typed", vec![EffectArgRef::String("value".into())])
            )
            .is_none()
        );
        for path in [
            vec![],
            vec!["root".into(), "".into()],
            vec!["bad\0segment".into()],
        ] {
            assert!(
                lower_external_action(
                    &registry,
                    &types.store,
                    &action("resource", vec![EffectArgRef::Path(path)])
                )
                .is_none()
            );
        }
        assert!(
            lower_external_action(
                &registry,
                &types.store,
                &action(
                    "typed",
                    vec![EffectArgRef::Type(etas_types::TypeId(u32::MAX))]
                )
            )
            .is_none()
        );
        assert!(
            lower_external_action(
                &registry,
                &types.store,
                &action(
                    "typed",
                    vec![EffectArgRef::Wildcard, EffectArgRef::Wildcard]
                )
            )
            .is_none()
        );
        for args in [vec![], vec![EffectArgRef::Wildcard]] {
            assert!(
                matches!(lower_external_action(&registry, &types.store, &action("typed", args.clone())), Some(Effect::AppliedAction(instance)) if instance.args == [EffectArgRef::Wildcard])
            );
            assert!(
                matches!(lower_external_action(&registry, &types.store, &action("resource", args)), Some(Effect::AppliedAction(instance)) if instance.args == [EffectArgRef::Wildcard])
            );
        }
    }

    #[test]
    fn callable_placeholders_and_alternatives_are_preserved_and_checked() {
        let span = Span::new(SourceId(7), TextRange::new(TextSize(0), TextSize(1)));
        let trace = ExternalActionTraceMetadata::Choice(vec![
            ExternalActionTraceMetadata::Seq(vec![ExternalActionTraceMetadata::ParameterCall {
                parameter: "callback".into(),
            }]),
            ExternalActionTraceMetadata::Widened {
                actions: vec![],
                parameter_calls: vec!["callback".into()],
            },
        ]);
        let parameters = vec![("callback".into(), ParameterCallTarget::Interface(2))];
        let lowered = lower_external_action_trace(&trace, &parameters, span, |_| None).unwrap();
        let ActionTraceDomain::Choice(branches) = lowered else {
            panic!("choice was lost")
        };
        assert!(
            matches!(&branches[0], ActionTraceDomain::Seq(nodes) if matches!(&nodes[0], ActionTraceDomain::ParameterCall { target: ParameterCallTarget::Interface(2), .. }))
        );
        assert!(
            matches!(&branches[1], ActionTraceDomain::Widened { parameter_calls, .. } if parameter_calls.iter().any(|call| call.target == ParameterCallTarget::Interface(2)))
        );
        assert!(lower_external_action_trace(&trace, &[], span, |_| None).is_none());
        assert!(
            lower_external_action_trace(
                &trace,
                &[
                    ("callback".into(), ParameterCallTarget::Interface(0)),
                    ("callback".into(), ParameterCallTarget::Interface(1))
                ],
                span,
                |_| None
            )
            .is_none()
        );
        assert!(
            lower_external_action_trace(
                &ExternalActionTraceMetadata::Choice(vec![]),
                &[],
                span,
                |_| None
            )
            .is_none()
        );
    }
}
