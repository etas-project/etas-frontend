use crate::{Determinism, EffectOutput, EffectUnit};

pub(crate) fn validate(
    hir: &etas_hir::HirRevision,
    effects: &mut EffectOutput,
    reachable: Option<&std::collections::BTreeSet<etas_hir::HirItemId>>,
) {
    let mut defaults = hir.parameter_defaults.iter().collect::<Vec<_>>();
    defaults.sort_by_key(|(parameter, _)| **parameter);
    for (&parameter, &expr) in defaults {
        if hir.parameter_injections.contains(&parameter) {
            continue;
        }
        let Some(etas_hir::SymbolDef::Param { owner, .. }) =
            hir.symbols.get(parameter).map(|symbol| &symbol.def)
        else {
            continue;
        };
        if reachable.is_some_and(|items| !items.contains(owner)) {
            continue;
        }
        let unit = EffectUnit::ParameterDefault {
            owner: *owner,
            parameter,
            expr,
        };
        let pure = effects
            .facts
            .unit_effects
            .get(&unit)
            .is_some_and(|summary| {
                summary.determinism == Determinism::Deterministic
                    && summary.escaping_effects.is_empty()
                    && summary.requested_actions.is_empty()
                    && summary.default_actions.is_empty()
                    && summary.handled_actions.is_empty()
                    && !matches!(summary.support, crate::InterpreterSupport::Rejected(_))
            });
        let immediate = effects
            .facts
            .suspension
            .get(&unit)
            .is_some_and(|summary| summary.proven_non_suspending());
        if !pure || !immediate {
            effects.diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::TypeMismatch,
                hir.exprs[expr].span(&hir.blocks),
                "parameter defaults must be proven deterministic, action-free and non-suspending",
            ));
        }
    }
}
