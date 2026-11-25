use crate::{EffectOutput, EffectUnit, suspension::SuspensionSummary};
use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirItem, HirReceiverMode, HirRevision};
use etas_types::{TypeOutput, method::MethodTarget};
use std::collections::BTreeMap;

pub(crate) fn validate(
    hir: &HirRevision,
    types: &TypeOutput,
    effects: &mut EffectOutput,
    imported: &BTreeMap<etas_code::CallableId, SuspensionSummary>,
    reachable_items: Option<&std::collections::BTreeSet<etas_hir::HirItemId>>,
) {
    for (item, declaration) in hir.items.iter() {
        if reachable_items.is_some_and(|reachable| !reachable.contains(&item)) {
            continue;
        }
        let HirItem::Flow(flow) = declaration else {
            continue;
        };
        if flow.receiver_mode != HirReceiverMode::Updating {
            continue;
        }
        if types.facts.type_impl_methods.values().any(|method| {
            method.body == item && types.facts.is_active_agent(&types.store, method.self_type)
        }) {
            continue;
        }
        let ordinary_method = types
            .facts
            .type_impl_methods
            .values()
            .any(|method| method.body == item);
        let summary = effects
            .facts
            .suspension
            .get(&EffectUnit::Item(item))
            .copied()
            .unwrap_or(SuspensionSummary::UNKNOWN);
        if !ordinary_method || !summary.proven_non_suspending() {
            effects.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                flow.span,
                if !ordinary_method {
                    "var self is restricted to ordinary type methods"
                } else {
                    "updating method requires a transitive non-suspension proof"
                },
            ));
        }
    }
    for (&call, method) in &types.facts.checked_method_calls {
        if reachable_items.is_some_and(|reachable| {
            hir.index()
                .expr_item
                .get(&call)
                .is_some_and(|owner| !reachable.contains(owner))
        }) {
            continue;
        }
        if !method.receiver.is_updating() {
            continue;
        }
        if method.kind.is_agent() {
            continue;
        }
        let span = hir.exprs[call].span(&hir.blocks);
        if let Err(reason) = etas_types::method::updating_receiver_symbol(hir, &types.facts, call) {
            effects.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                span,
                reason,
            ));
        }
        let summary = match &method.target {
            MethodTarget::Source { callable, .. } => effects
                .facts
                .suspension
                .get(&EffectUnit::Item(*callable))
                .copied(),
            MethodTarget::External { identity } => imported
                .get(&etas_code::CallableId::method(identity.clone()))
                .copied(),
            _ => None,
        }
        .unwrap_or(SuspensionSummary::UNKNOWN);
        if !summary.proven_non_suspending() {
            effects.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                span,
                "updating call requires a checked non-suspending target",
            ));
        }
    }
}
