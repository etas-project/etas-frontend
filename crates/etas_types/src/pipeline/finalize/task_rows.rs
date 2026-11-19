use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use etas_hir::{HirArg, HirExpr, HirExprId, HirProgram, ResolveResult, SymbolDef};

use crate::{
    CallableGenericParamKind, CallableSignature, EffectRowRef, SymbolTypeFact, Type, TypeId,
    TypeOutput,
};

use super::ResolvedEffectRowInstantiation;

/// Publish solved task-result rows along their checked local value bindings.
/// A template TypeId may be shared by different starts; refinements therefore
/// belong to expressions and symbols, never a global `E` or TypeId replacement.
pub fn refine_task_value_rows(
    hir: &HirProgram,
    output: &mut TypeOutput,
    solved: &[ResolvedEffectRowInstantiation],
) -> Result<bool, String> {
    if !output.facts.generic_instantiations.keys().any(|call| {
        output.facts.expr_types.get(call).is_some_and(|ty| {
            crate::source_execution::task(&output.store, &output.facts.known_std_types, *ty)
                .is_some()
        })
    }) {
        return Ok(false);
    }
    let known = output.facts.known_std_types.clone();
    let mut next = output.clone();
    let mut interner = crate::TypeInterner::from_store(next.store.as_ref().clone());
    let facts = Arc::make_mut(&mut next.facts);
    let solved = solved
        .iter()
        .map(|value| (value.call, &value.bindings))
        .collect::<HashMap<_, _>>();
    let mut changed_any = false;
    loop {
        let mut changed = false;
        let mut calls = facts
            .generic_instantiations
            .keys()
            .copied()
            .collect::<Vec<_>>();
        calls.sort();
        for call in &calls {
            let Some(&old) = facts.expr_types.get(call) else {
                continue;
            };
            if !crate::source_execution::task(interner.store(), &known, old)
                .is_some_and(|task| task.effects.tail.is_some())
            {
                continue;
            }
            let fact = &facts.generic_instantiations[call];
            let mut rows = fact
                .effect_row_bindings
                .iter()
                .cloned()
                .collect::<HashMap<_, _>>();
            if let Some(solved) = solved.get(call) {
                rows.extend(solved.iter().filter(|(_, row)| row.tail.is_none()).cloned());
            }
            let names = HashMap::new();
            let ty = crate::TypeSubstitutionEngine::materializing(&mut interner, &names, None)
                .with_effect_arguments(&rows)
                .substitute(old)
                .map_err(|error| error.to_string())?;
            if crate::source_execution::task(interner.store(), &known, ty)
                .is_some_and(|task| task.effects.tail.is_none())
                && ty != old
            {
                facts.expr_types.insert(*call, ty);
                changed = true;
            }
        }
        // Local aliases retain the value origin. An explicit closed contract is
        // kept; only an unsolved task row can acquire this checked refinement.
        loop {
            let mut local_change = false;
            for symbol in hir.symbols.iter() {
                let SymbolDef::Local {
                    initializer: Some(source),
                    ..
                } = symbol.def
                else {
                    continue;
                };
                let Some(&ty) = facts.expr_types.get(&source) else {
                    continue;
                };
                if !crate::source_execution::task(interner.store(), &known, ty)
                    .is_some_and(|task| task.effects.tail.is_none())
                {
                    continue;
                }
                if let Some(SymbolTypeFact::Local {
                    ty: old,
                    mutable: false,
                }) = facts.symbol_types.get_mut(&symbol.id)
                    && task_refines(interner.store(), &known, *old, ty)
                {
                    *old = ty;
                    local_change = true;
                }
            }
            for (expr, node) in hir.exprs.iter() {
                let HirExpr::Path(path) = node else { continue };
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    continue;
                };
                let Some(SymbolTypeFact::Local { ty, .. }) = facts.symbol_types.get(&symbol) else {
                    continue;
                };
                let Some(old) = facts.expr_types.get_mut(&expr) else {
                    continue;
                };
                if task_refines(interner.store(), &known, *old, *ty) {
                    *old = *ty;
                    local_change = true;
                }
            }
            if !local_change {
                break;
            }
            changed = true;
        }
        for call in calls {
            let Some((mut signature, args)) = call_signature(hir, facts, call) else {
                continue;
            };
            // A producer may already have its final row before this pass.
            // Its action consumers still need their own declaration binders
            // solved from that closed Task value.
            if !args.iter().any(|expr| {
                facts.expr_types.get(expr).is_some_and(|ty| {
                    crate::source_execution::task(interner.store(), &known, *ty)
                        .is_some_and(|task| task.effects.tail.is_none())
                })
            }) {
                continue;
            }
            let pairs = signature
                .params
                .iter()
                .copied()
                .zip(
                    args.iter()
                        .filter_map(|expr| facts.expr_types.get(expr).copied()),
                )
                .collect::<Vec<_>>();
            if pairs.len() != signature.params.len() {
                return Err("task row refinement lost its checked argument ABI".into());
            }
            let params = signature
                .generic_params
                .iter()
                .filter(|param| param.kind == CallableGenericParamKind::Effect)
                .map(|param| param.name.clone())
                .collect::<Vec<_>>();
            if params.is_empty() {
                continue;
            }
            let fact = &facts.generic_instantiations[&call];
            let names = fact.type_bindings.iter().cloned().collect();
            let mut rows = fact
                .effect_row_bindings
                .iter()
                .filter(|(_, row)| row.tail.is_none())
                .cloned()
                .collect();
            let inference = crate::solver::effect_row::infer_bindings_from_type_pairs(
                interner.store(),
                &pairs,
                &names,
                &crate::Substitution::default(),
                crate::solver::effect_row::EffectRowParameterScopes {
                    inferred: &params,
                    lexical: &[],
                },
                &mut rows,
            );
            if inference.iter().any(|value| !value.matches) {
                return Err("refined task row conflicts with its checked call contract".into());
            }
            let deferred = inference
                .iter()
                .flat_map(|value| &value.deferred)
                .collect::<HashSet<_>>();
            let rows = rows
                .into_iter()
                .filter(|(name, row)| row.tail.is_none() && !deferred.contains(name))
                .collect::<HashMap<String, EffectRowRef>>();
            if rows.is_empty() {
                continue;
            }
            let fact = facts.generic_instantiations.get_mut(&call).unwrap();
            let mut binding_change = false;
            for name in &params {
                let Some(row) = rows.get(name) else { continue };
                if let Some((_, old)) = fact
                    .effect_row_bindings
                    .iter_mut()
                    .find(|(bound, _)| bound == name)
                {
                    if old != row {
                        *old = row.clone();
                        binding_change = true;
                    }
                } else {
                    fact.effect_row_bindings.push((name.clone(), row.clone()));
                    binding_change = true;
                }
            }
            let before = fact.deferred_effect_row_obligations.len();
            fact.deferred_effect_row_obligations
                .retain(|obligation| !rows.contains_key(&obligation.param));
            binding_change |= before != fact.deferred_effect_row_obligations.len();
            if !binding_change {
                continue;
            }
            let mut engine =
                crate::TypeSubstitutionEngine::materializing(&mut interner, &names, None)
                    .with_effect_arguments(&rows);
            for ty in signature
                .params
                .iter_mut()
                .chain(std::iter::once(&mut signature.output))
            {
                *ty = engine.substitute(*ty).map_err(|error| error.to_string())?;
            }
            signature.effects = signature
                .effects
                .map(|row| engine.substitute_effect_row(row))
                .transpose()
                .map_err(|error| error.to_string())?;
            signature.requested_actions = signature
                .requested_actions
                .map(|row| engine.substitute_effect_row(row))
                .transpose()
                .map_err(|error| error.to_string())?;
            if let Some(action) = facts.action_calls.get_mut(&call) {
                action.signature = signature;
            } else if let Some(method) = facts.checked_method_calls.get_mut(&call) {
                method.signature = signature;
                method.instantiation.effect_row_bindings = fact.effect_row_bindings.clone();
                method.instantiation.deferred_effect_row_obligations =
                    fact.deferred_effect_row_obligations.clone();
            } else if let Some(native) = facts.std_intrinsic_calls.get_mut(&call) {
                native.signature = signature;
                native.callable_type = interner.intern(Type::Function(crate::FlowType {
                    input: native.signature.params.clone(),
                    output: native.signature.output,
                    effects: native.signature.effects.clone(),
                }));
            }
            changed = true;
        }
        if !changed {
            break;
        }
        changed_any = true;
    }
    if changed_any {
        next.store = Arc::new(interner.into_store());
        *output = next;
    }
    Ok(changed_any)
}

fn task_refines(
    store: &crate::TypeStore,
    known: &crate::KnownStdTypes,
    old: TypeId,
    new: TypeId,
) -> bool {
    if matches!(store.get(old), Some(Type::Task(_)))
        != matches!(store.get(new), Some(Type::Task(_)))
    {
        return false;
    }
    matches!((crate::source_execution::task(store, known, old), crate::source_execution::task(store, known, new)),
        (Some(old), Some(new)) if old.output == new.output && old.effects.tail.is_some() && new.effects.tail.is_none())
}

fn call_signature(
    hir: &HirProgram,
    facts: &crate::TypeFacts,
    call: HirExprId,
) -> Option<(CallableSignature, Vec<HirExprId>)> {
    if let Some(method) = facts.checked_method_calls.get(&call) {
        let args = (0..method.signature.params.len())
            .map(|slot| crate::method::argument_expression(hir, call, method, slot as u32))
            .collect::<Option<Vec<_>>>()?;
        return Some((method.signature.clone(), args));
    }
    let args = match hir.exprs.get(call)? {
        HirExpr::Call { args, .. } | HirExpr::Perform { args, .. } => args,
        _ => return None,
    };
    let args = args
        .iter()
        .map(|arg| match arg {
            HirArg::Positional(expr) => Some(*expr),
            HirArg::Named { .. } => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let signature = if let Some(action) = facts.action_calls.get(&call) {
        action.signature.clone()
    } else if let Some(native) = facts.std_intrinsic_calls.get(&call) {
        // The checked payload ABI may already contain a closed producer row
        // while its generic obligation still refers to the original argument.
        // Infer that binder from the declaration, before effect analysis can
        // reconstruct an equivalent row with a different registry spelling.
        flow_signature(hir, facts, call).unwrap_or_else(|| native.signature.clone())
    } else if let HirExpr::Perform { action, .. } = hir.exprs.get(call)? {
        let ResolveResult::Resolved(symbol) = action.action_symbol else {
            return None;
        };
        let action = facts.action_signatures.get(&symbol)?;
        CallableSignature {
            parameter_interface: action.parameter_interface.clone(),
            generic_params: action.generic_params.clone(),
            params: action.params.clone(),
            output: action.output,
            effects: action.default_handler_effects.clone(),
            requested_actions: None,
        }
    } else {
        flow_signature(hir, facts, call)?
    };
    Some((signature, args))
}

fn flow_signature(
    hir: &HirProgram,
    facts: &crate::TypeFacts,
    call: HirExprId,
) -> Option<CallableSignature> {
    let HirExpr::Call { callee, .. } = hir.exprs[call] else {
        return None;
    };
    let HirExpr::Path(path) = &hir.exprs[callee] else {
        return None;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = hir.canonical_source_symbol(symbol).unwrap_or(symbol);
    let SymbolTypeFact::Flow { signature } = facts.symbol_types.get(&symbol)? else {
        return None;
    };
    Some(signature.clone())
}
