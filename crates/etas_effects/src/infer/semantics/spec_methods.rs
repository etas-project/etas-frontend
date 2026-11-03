use etas_hir::{HirExpr, HirExprId, HirItem, ResolveResult, SymbolDef};
use etas_types::TypeUnifier;

use super::{
    engine::EffectSemantics,
    specialize::{CheckedGenericBindings, EffectSpecializationError},
};
use crate::infer::unit::EffectUnit;

mod dependencies;
mod evidence;
mod implementations;
mod metadata;
mod metadata_graphs;
mod rows;
pub(super) use evidence::MethodEvidence;
pub(crate) use metadata_graphs::MethodCallbackGraph;

fn deferred_completion(
    summary: &mut crate::EffectSummary,
    target: crate::ParameterCallTarget,
    output: etas_types::TypeId,
    span: etas_core::Span,
) {
    // This possible return is conditional on the explicit completion obligation;
    // an effect bound alone never certifies that an implementation returns.
    let mut exits = crate::CallableExitSummary::default();
    exits.record(
        crate::CallableOutcome::Returned {
            ty: output,
            observation: crate::ReturnObservation::Any,
        },
        summary.action_trace.clone(),
    );
    exits.gaps.insert(crate::OutcomeAnalysisGap::ParameterCall {
        target,
        output,
        source: span.into(),
    });
    summary.exits = Some(exits);
}

impl EffectSemantics<'_> {
    pub(super) fn bind_spec_method_generic_parameters(
        &self,
        call: HirExprId,
        bindings: &mut CheckedGenericBindings,
    ) -> Result<(), EffectSpecializationError> {
        let Some(HirExpr::SpecMethodCall {
            spec_path, method, ..
        }) = self.hir.exprs.get(call)
        else {
            return Ok(());
        };
        let invalid = || EffectSpecializationError::InvalidCallableParameterInterface;
        let ResolveResult::Resolved(spec) = spec_path.resolution else {
            return Err(invalid());
        };
        let spec = self
            .type_symbols
            .canonical_symbol(self.hir, spec)
            .ok_or_else(invalid)?;
        let method = self
            .types
            .facts
            .spec_signatures
            .get(&spec)
            .and_then(|spec| {
                spec.methods
                    .iter()
                    .find(|candidate| candidate.name == *method)
            })
            .ok_or_else(invalid)?;
        let declaration = method.signature_in(&self.types.facts).ok_or_else(invalid)?;
        let Some(EffectUnit::Item(item)) = self.checked_spec_method_target(call) else {
            return Err(invalid());
        };
        let Some(etas_types::ItemSignature::Flow(implementation)) =
            self.types.facts.item_signatures.get(&item)
        else {
            return Err(invalid());
        };
        let arguments = etas_types::GenericInstantiationFact {
            type_bindings: bindings.types.clone(),
            effect_row_bindings: bindings.effect_rows.clone(),
            deferred_effect_row_obligations: bindings.deferred_effect_rows.clone(),
        };
        let arguments = method
            .implementation_instantiation(declaration, implementation, &arguments)
            .map_err(|reason| EffectSpecializationError::InvalidCallableEnvironment { reason })?;
        bindings.types = arguments.type_bindings;
        bindings.effect_rows = arguments.effect_row_bindings;
        bindings.deferred_effect_rows = arguments.deferred_effect_row_obligations;
        Ok(())
    }

    pub(super) fn checked_spec_method_target(&self, call: HirExprId) -> Option<EffectUnit> {
        let HirExpr::SpecMethodCall {
            receiver,
            spec_path,
            spec_args,
            method,
            ..
        } = self.hir.exprs.get(call)?
        else {
            return None;
        };
        let ResolveResult::Resolved(spec_symbol) = spec_path.resolution else {
            return None;
        };
        let spec_symbol = self.type_symbols.canonical_symbol(self.hir, spec_symbol)?;
        let receiver = *self.types.facts.expr_types.get(receiver)?;
        let args = spec_args
            .iter()
            .map(|arg| self.types.facts.type_refs.get(arg).copied())
            .collect::<Option<Vec<_>>>()?;
        self.spec_method_target_for_types(spec_symbol, receiver, &args, method)
    }

    pub(super) fn spec_method_target_for_types(
        &self,
        spec_symbol: etas_hir::SymbolId,
        receiver: etas_types::TypeId,
        args: &[etas_types::TypeId],
        method: &str,
    ) -> Option<EffectUnit> {
        let same = |left, right| {
            let mut unifier = TypeUnifier::new(&self.types.store);
            unifier.unify(left, right).is_ok() && unifier.substitution().iter().next().is_none()
        };
        let mut targets = self
            .types
            .facts
            .spec_impls
            .iter()
            .filter(|implementation| {
                implementation.spec_symbol == spec_symbol
                    && same(implementation.self_type, receiver)
                    && implementation.args.len() == args.len()
                    && implementation
                        .args
                        .iter()
                        .zip(args)
                        .all(|(left, right)| same(*left, *right))
            })
            .flat_map(|implementation| &implementation.methods)
            .filter(|candidate| candidate.name == method)
            .filter_map(
                |candidate| match self.hir.symbols.get(candidate.symbol)?.def {
                    SymbolDef::Item { item }
                        if matches!(self.hir.items.get(item), Some(HirItem::Flow(_))) =>
                    {
                        Some(EffectUnit::Item(item))
                    }
                    _ => None,
                },
            )
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        match targets.as_slice() {
            [target] => Some(*target),
            _ => None,
        }
    }
}
