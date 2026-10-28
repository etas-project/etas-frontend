use crate::trace_spec::model::{TraceSpecClause, TraceSpecClauseAlternatives, TraceSpecPattern};
use crate::{Effect, EffectRow, TraceSpecClauseFact};
use etas_types::{EffectArgRef, SpecSignature, TypeId, TypeStore};
use std::collections::HashMap;

pub(super) fn instantiate(
    alternatives: &mut TraceSpecClauseAlternatives,
    signature: &SpecSignature,
    args: &[TypeId],
    store: &TypeStore,
) -> Result<(), String> {
    if args.len() != signature.params.len() || signature.param_names.len() != signature.params.len()
    {
        return Err("checked trace spec generic parameter arity is inconsistent".into());
    }
    let bindings = signature
        .param_names
        .iter()
        .cloned()
        .zip(args.iter().copied())
        .collect::<HashMap<_, _>>();
    if bindings.is_empty() {
        return Ok(());
    }
    for clause in alternatives.iter_mut().flatten() {
        match clause {
            TraceSpecClause::Allow { pattern, fact, .. } => {
                instantiate_pattern(pattern, &bindings, store)?;
                *fact = TraceSpecClauseFact::Allow {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
            }
            TraceSpecClause::Deny { pattern, fact, .. } => {
                instantiate_pattern(pattern, &bindings, store)?;
                *fact = TraceSpecClauseFact::Deny {
                    pattern: pattern.row.clone(),
                    label: pattern.label.clone(),
                };
            }
            TraceSpecClause::RequireBefore {
                guard,
                target,
                fact,
                ..
            } => {
                instantiate_pattern(guard, &bindings, store)?;
                instantiate_pattern(target, &bindings, store)?;
                *fact = TraceSpecClauseFact::RequireBefore {
                    guard: guard.row.clone(),
                    guard_label: guard.label.clone(),
                    target: target.row.clone(),
                    target_label: target.label.clone(),
                };
            }
            TraceSpecClause::RequireAfter {
                target,
                obligation,
                fact,
                ..
            } => {
                instantiate_pattern(target, &bindings, store)?;
                instantiate_pattern(obligation, &bindings, store)?;
                *fact = TraceSpecClauseFact::RequireAfter {
                    target: target.row.clone(),
                    target_label: target.label.clone(),
                    obligation: obligation.row.clone(),
                    obligation_label: obligation.label.clone(),
                };
            }
            TraceSpecClause::Limit { .. } => {}
        }
    }
    Ok(())
}

fn instantiate_pattern(
    pattern: &mut TraceSpecPattern,
    bindings: &HashMap<String, TypeId>,
    store: &TypeStore,
) -> Result<(), String> {
    let ty = |ty| {
        etas_types::substitute_named_params_in_store(store, ty, bindings)
            .map_err(|error| error.to_string())
    };
    let effects = pattern
        .row
        .effects
        .iter()
        .map(|effect| {
            Ok(match effect {
                Effect::Error(error) => Effect::Error(ty(*error)?),
                Effect::Applied { tag, args } => Effect::Applied {
                    tag: *tag,
                    args: args.iter().map(|arg| ty(*arg)).collect::<Result<_, _>>()?,
                },
                Effect::AppliedAction(instance) => {
                    let mut instance = instance.clone();
                    for arg in &mut instance.args {
                        if let EffectArgRef::Type(value) = arg {
                            *value = ty(*value)?;
                        }
                    }
                    Effect::AppliedAction(instance)
                }
                other => other.clone(),
            })
        })
        .collect::<Result<crate::EffectSet, String>>()?;
    pattern.row = EffectRow {
        effects,
        open: pattern.row.open,
    };
    for bound in &mut pattern.arg_bounds {
        for arg in &mut bound.spec_args {
            *arg = ty(*arg)?;
        }
    }
    Ok(())
}
