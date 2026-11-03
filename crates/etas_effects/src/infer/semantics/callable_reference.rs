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
            LatentSource::ExternalFrozen { reflection } => {
                self.external_frozen_summary(reflection, span)?
            }
            LatentSource::ExternalReference { value, .. } => {
                self.external_reference_summary(value, span)?
            }
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
        let mut signature = *self.types.facts.expr_types.get(&value)?;
        if self.types.facts.reflections.contains_key(&value) {
            signature = self.descriptor_contract_type(signature)?;
        }
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
        for expr in self.types.facts.expr_types.keys() {
            if !matches!(self.hir.exprs.get(*expr), Some(HirExpr::Path(_))) {
                continue;
            }
            let Some(callable) = self
                .external_reference_metadata(*expr)
                .and_then(|metadata| metadata.callable_contract.as_ref())
                .map(|contract| contract.header.id.clone())
            else {
                continue;
            };
            for frozen in [false, true] {
                if let Some(key) = self.callable_reference_key(
                    CallableReferenceTarget::External {
                        callable: callable.clone(),
                        frozen,
                        reflection: false,
                    },
                    *expr,
                ) {
                    self.callable_references.insert(
                        key,
                        LatentSource::ExternalReference {
                            value: *expr,
                            frozen,
                        },
                    );
                }
            }
        }
        for (expr, fact) in &self.types.facts.reflections {
            if let etas_types::CheckedReflectionTarget::ExternalCallable { callable, .. } =
                &fact.target
                && self.external_reflection_metadata(*expr).is_some()
                && let Some(key) = self.callable_reference_key(
                    CallableReferenceTarget::External {
                        callable: callable.clone(),
                        frozen: true,
                        reflection: true,
                    },
                    *expr,
                )
            {
                self.callable_references
                    .insert(key, LatentSource::ExternalFrozen { reflection: *expr });
            }
        }
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
            let symbol = match self.hir.exprs.get(*expr) {
                Some(HirExpr::Path(path)) => match path.resolution {
                    ResolveResult::Resolved(symbol) => symbol,
                    _ => continue,
                },
                Some(HirExpr::Reflect(_)) => match self.types.facts.reflections.get(expr) {
                    Some(etas_types::CheckedReflectionFact {
                        target: etas_types::CheckedReflectionTarget::Callable { declaration },
                        ..
                    }) => *declaration,
                    _ => continue,
                },
                _ => continue,
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
