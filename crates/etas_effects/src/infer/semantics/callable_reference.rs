use etas_hir::{HirExpr, HirExprId, HirItem, ResolveResult, SymbolDef};
use etas_types::Type;

use super::specialize::EffectSpecializationError;
use super::{engine::EffectSemantics, state::LatentSource};
use crate::{CallableReferenceTarget, CallableReturnReference, EffectUnit};

pub(in crate::infer::semantics) mod environment;

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

impl EffectSemantics<'_> {
    pub(super) fn reference_summary(
        &self,
        source: LatentSource,
        span: etas_core::Span,
    ) -> Result<crate::EffectSummary, EffectSpecializationError> {
        let reference = self
            .callable_reference_for_source(source)
            .ok_or_else(|| invalid("callable reference has no checked signature"))?;
        let summary = match source {
            LatentSource::Reference { unit, .. } => {
                let summary = self
                    .latent_leaf_summary(LatentSource::Unit(unit), span)
                    .ok_or_else(|| invalid("referenced source callable has no solved summary"))?;
                let rows = reference
                    .effect_row_bindings
                    .iter()
                    .map(|(name, row)| (name.clone(), self.row_from_type_ref(row)))
                    .collect::<Vec<_>>();
                self.require_effect_type_bindings(unit, &summary, &reference.type_bindings)?;
                self.require_effect_row_bindings(unit, &summary, &rows)?;
                self.specialize_summary_with_bindings(
                    &summary,
                    &[],
                    &reference.type_bindings,
                    &rows,
                )?
            }
            LatentSource::StdReference { value, .. } => self
                .inputs
                .std_callable_values
                .get(&value)
                .ok_or_else(|| invalid("standard reference has no checked intrinsic summary"))?
                .summary
                .clone(),
            _ => {
                return Err(invalid(
                    "summary target is not a checked callable reference",
                ));
            }
        };
        let (types, rows) = self.reference_environment_bindings(&reference)?;
        self.specialize_summary_with_bindings(&summary, &[], &types, &rows)
    }
}

impl EffectSemantics<'_> {
    pub(super) fn callable_reference_key(
        &self,
        target: CallableReferenceTarget,
        value: HirExprId,
    ) -> Option<CallableReturnReference> {
        let signature = *self.types.facts.expr_types.get(&value)?;
        if !matches!(self.types.store.get(signature), Some(Type::Function(_))) {
            return None;
        }
        let fact = self.types.facts.generic_instantiations.get(&value);
        if fact.is_some_and(|fact| !fact.deferred_effect_row_obligations.is_empty()) {
            return None;
        }
        let mut type_bindings = fact
            .map(|fact| fact.type_bindings.clone())
            .unwrap_or_default();
        let mut effect_row_bindings = fact
            .map(|fact| fact.effect_row_bindings.clone())
            .unwrap_or_default();
        type_bindings.sort();
        effect_row_bindings.sort();
        Some(CallableReturnReference {
            value,
            owner: *self.context.view().index().expr_item.get(&value)?,
            environment: None,
            target,
            signature,
            type_bindings,
            effect_row_bindings,
        })
    }

    pub(super) fn collect_callable_references(&mut self) {
        for (expr, fact) in &self.inputs.std_callable_values {
            if let Some(key) = self.callable_reference_key(
                CallableReferenceTarget::StdIntrinsic(fact.intrinsic),
                *expr,
            ) {
                self.callable_references.insert(
                    key,
                    LatentSource::StdReference {
                        value: *expr,
                        environment: None,
                    },
                );
            }
        }
        for expr in self.types.facts.generic_instantiations.keys() {
            let Some(HirExpr::Path(path)) = self.hir.exprs.get(*expr) else {
                continue;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                continue;
            };
            let Some(symbol) = self.type_symbols.canonical_symbol(self.hir, symbol) else {
                continue;
            };
            let Some(SymbolDef::Item { item }) =
                self.hir.symbols.get(symbol).map(|symbol| &symbol.def)
            else {
                continue;
            };
            if !matches!(
                self.hir.items.get(*item),
                Some(HirItem::Flow(_) | HirItem::Tool(_))
            ) {
                continue;
            }
            if let Some(key) =
                self.callable_reference_key(CallableReferenceTarget::Source(*item), *expr)
            {
                self.inputs
                    .native_call_targets
                    .entry(*expr)
                    .or_default()
                    .insert(EffectUnit::Item(*item));
                self.callable_references.insert(
                    key,
                    LatentSource::Reference {
                        unit: EffectUnit::Item(*item),
                        value: *expr,
                        environment: None,
                    },
                );
            }
        }
    }
}
