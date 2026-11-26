//! A source cursor stores one next callable. Follow the callable returned in
//! every tail as well as the first one; an empty escaping row is not a proof.
use std::collections::BTreeSet;

use etas_types::{ItemSignature, Type, TypeId};

use super::{CallContext, EffectUnit, HirExprId, Instance, Node, SuspensionSummary, instances};
use crate::{
    CallableOutcome, CallableReferenceTarget, CallableReturnSource, ReturnObservation,
    facts::{CallableEnvironmentId, CallableValueOrigin},
};

pub(super) fn begin(context: CallContext<'_>, iterable: HirExprId, node: &mut Node) {
    let Some(ty) = instances::expression_type(context, iterable) else {
        node.local.join(SuspensionSummary::UNKNOWN);
        return;
    };
    if context
        .types
        .store
        .get(ty)
        .is_some_and(super::storage_iterable)
    {
        return;
    }
    if !matches!(
        etas_types::source_iteration::checked_cursor_indexed(
            &context.types.store,
            &context.types.facts,
            context.type_lookup,
            ty
        ),
        Ok(Some(_))
    ) {
        node.local.join(SuspensionSummary::UNKNOWN);
        return;
    }
    let Some(sources) = context.effects.iteration_call_sources.get(&iterable) else {
        node.local.join(SuspensionSummary::UNKNOWN);
        return;
    };
    if sources.is_empty() {
        node.local.join(SuspensionSummary::UNKNOWN);
    }
    let mut seen = BTreeSet::new();
    let mut next = Node::default();
    for source in sources {
        origin(context, *source, ty, &mut next, &mut seen);
    }
    if next.calls.is_empty() {
        next.local.join(SuspensionSummary::UNKNOWN);
    }
    node.local.join(next.local);
    node.calls.extend(next.calls);
}

pub(super) fn successors(
    context: CallContext<'_>,
    unit: EffectUnit,
    cursor: TypeId,
    node: &mut Node,
) {
    let Some(exits) = instances::checked_summary(context.effects, unit, context.environment)
        .and_then(|summary| summary.exits.as_ref())
    else {
        node.local.join(SuspensionSummary::UNKNOWN);
        return;
    };
    if exits.gaps.iter().any(|gap| {
        matches!(
            gap,
            crate::OutcomeAnalysisGap::MissingReturnType
                | crate::OutcomeAnalysisGap::MissingReturnObservation
        )
    }) {
        // Callback completion can be unknown even when its execution is
        // non-suspending. The invocation graph proves execution separately;
        // the checked tail projections below prove every returned next value.
        node.local.join(SuspensionSummary::UNKNOWN);
    }
    let mut seen = BTreeSet::new();
    let tail = vec![
        etas_code::descriptor::ValueProjectionStep::VariantPayload {
            variant: "Some".into(),
            position: 0,
        },
        etas_code::descriptor::ValueProjectionStep::TupleElement(1),
        etas_code::descriptor::ValueProjectionStep::Field("next".into()),
    ];
    match exits.value_projections.get(&tail) {
        Some(crate::ProjectedReturnSources::Known(sources)) => {
            let mut tails = Node::default();
            for source in sources {
                match source {
                    CallableReturnSource::AnonymousFlow(unit) => {
                        step(context, *unit, None, None, cursor, &mut tails);
                    }
                    CallableReturnSource::Captured { unit, environment } => {
                        step(context, *unit, Some(*environment), None, cursor, &mut tails);
                    }
                    CallableReturnSource::SourceItem(item) => {
                        step(
                            context,
                            EffectUnit::Item(*item),
                            None,
                            None,
                            cursor,
                            &mut tails,
                        );
                    }
                    CallableReturnSource::Reference(reference) => {
                        if let CallableReferenceTarget::Source(item) = reference.target {
                            step(
                                context,
                                EffectUnit::Item(item),
                                reference.environment,
                                Some(reference.value),
                                cursor,
                                &mut tails,
                            );
                        } else {
                            tails.local.join(SuspensionSummary::UNKNOWN);
                        }
                    }
                    CallableReturnSource::Parameter(symbol) => {
                        parameter(context, *symbol, cursor, &mut tails, &mut seen);
                    }
                    CallableReturnSource::Projection(id) => {
                        let sources = instances::resolve(
                            context,
                            &BTreeSet::from([CallableValueOrigin::Projection(*id)]),
                        );
                        for source in sources {
                            origin(context, source, cursor, &mut tails, &mut seen);
                        }
                    }
                    _ => {
                        tails.local.join(SuspensionSummary::UNKNOWN);
                    }
                }
            }
            if !sources.is_empty() && tails.calls.is_empty() {
                tails.local.join(SuspensionSummary::UNKNOWN);
            }
            node.local.join(tails.local);
            node.calls.extend(tails.calls);
        }
        Some(
            crate::ProjectedReturnSources::Unavailable
            | crate::ProjectedReturnSources::InjectedField(_),
        ) => {
            node.local.join(SuspensionSummary::UNKNOWN);
        }
        None => {
            for exit in &exits.exits {
                match &exit.outcome {
                    CallableOutcome::Returned {
                        observation: ReturnObservation::Option(false),
                        ..
                    } => {}
                    CallableOutcome::Returned {
                        observation: ReturnObservation::Variant(symbol),
                        ..
                    } if empty_option(context, *symbol) => {}
                    CallableOutcome::Returned { .. } => {
                        node.local.join(SuspensionSummary::UNKNOWN);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn origin(
    context: CallContext<'_>,
    source: CallableValueOrigin,
    cursor: TypeId,
    node: &mut Node,
    seen: &mut BTreeSet<CallableValueOrigin>,
) {
    if !seen.insert(source) {
        node.local.join(SuspensionSummary::UNKNOWN);
        return;
    }
    match source {
        CallableValueOrigin::Unit(unit) => step(context, unit, None, None, cursor, node),
        CallableValueOrigin::Captured { unit, environment } => {
            step(context, unit, Some(environment), None, cursor, node);
        }
        CallableValueOrigin::Reference {
            unit,
            value,
            environment,
        } => {
            step(context, unit, environment, Some(value), cursor, node);
        }
        CallableValueOrigin::Parameter { symbol, .. } => {
            parameter(context, symbol, cursor, node, seen);
        }
        _ => {
            node.local.join(SuspensionSummary::UNKNOWN);
        }
    }
    seen.remove(&source);
}

fn parameter(
    context: CallContext<'_>,
    symbol: etas_hir::SymbolId,
    cursor: TypeId,
    node: &mut Node,
    seen: &mut BTreeSet<CallableValueOrigin>,
) {
    // A generic aggregate's origins include its data element as well as its
    // cursor. Specialization can establish that this parameter carries data
    // only; it must not then be invoked as a tail's next callback.
    if let Some(etas_types::SymbolTypeFact::Param { ty }) =
        context.types.facts.symbol_types.get(&symbol)
        && let Some(ty) = instances::substitute_type(context, *ty)
        && !context.payload_shapes.contains(context.types, ty)
        && !context.payload_shapes.has_parameters(context.types, ty)
    {
        return;
    }
    let source = CallableValueOrigin::Parameter {
        owner: match context.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
            Some(etas_hir::SymbolDef::Param { owner, .. }) => *owner,
            _ => {
                node.local.join(SuspensionSummary::UNKNOWN);
                return;
            }
        },
        symbol,
    };
    let sources = instances::resolve(context, &BTreeSet::from([source]));
    if sources.is_empty() || sources.contains(&source) {
        node.local.join(SuspensionSummary::UNKNOWN);
    }
    for resolved in sources {
        if resolved != source {
            origin(context, resolved, cursor, node, seen);
        }
    }
}

fn step(
    context: CallContext<'_>,
    unit: EffectUnit,
    environment: Option<CallableEnvironmentId>,
    reference: Option<HirExprId>,
    cursor: TypeId,
    node: &mut Node,
) {
    let instance = (|| {
        let mut instance = if let Some(reference) = reference {
            instances::source(context, reference, unit)?
        } else {
            Instance {
                unit,
                bindings: context.bindings.to_vec(),
                environment: None,
                cursor: None,
                values: context.values.clone(),
            }
        };
        if let Some(environment) = environment {
            instance = instances::captured(context, instance, environment)?;
        }
        let call = CallContext {
            bindings: &instance.bindings,
            environment: instance.environment,
            values: &instance.values,
            ..context
        };
        let (input_empty, output) = match unit {
            EffectUnit::AnonymousFlow { value, .. } => {
                let Type::Function(signature) = context
                    .types
                    .store
                    .get(*context.types.facts.expr_types.get(&value)?)?
                else {
                    return None;
                };
                (signature.input.is_empty(), signature.output)
            }
            EffectUnit::Item(item) => {
                let ItemSignature::Flow(signature) =
                    context.types.facts.item_signatures.get(&item)?
                else {
                    return None;
                };
                (signature.params.is_empty(), signature.output)
            }
            _ => return None,
        };
        let layout = etas_types::source_iteration::checked_cursor_indexed(
            &context.types.store,
            &context.types.facts,
            context.type_lookup,
            cursor,
        )
        .ok()??;
        if !input_empty
            || instances::substitute_type(call, output)? != layout.step
            || !context.effects.unit_effects.contains_key(&unit)
        {
            return None;
        }
        instance.cursor = Some(cursor);
        Some(instance)
    })();
    if let Some(instance) = instance {
        node.calls.insert(instance);
    } else {
        node.local.join(SuspensionSummary::UNKNOWN);
    }
}

fn empty_option(context: CallContext<'_>, symbol: etas_hir::SymbolId) -> bool {
    let Some(symbol) = context.hir.canonical_source_symbol(symbol) else {
        return false;
    };
    let Some(etas_types::SymbolTypeFact::Flow { signature }) =
        context.types.facts.symbol_types.get(&symbol)
    else {
        return false;
    };
    let Some(Type::Applied { constructor, .. }) = context.types.store.get(signature.output) else {
        return false;
    };
    if context.types.facts.known_std_types.option_enum != Some(TypeId(constructor.0)) {
        return false;
    }
    let Some(etas_hir::SymbolDef::EnumVariant { variant_index, .. }) =
        context.hir.symbols.get(symbol).map(|symbol| &symbol.def)
    else {
        return false;
    };
    context
        .types
        .facts
        .enum_layouts
        .get(&TypeId(constructor.0))
        .and_then(|layout| layout.variants.get(*variant_index as usize))
        .is_some_and(|variant| variant.name == "None" && variant.fields.is_empty())
}
