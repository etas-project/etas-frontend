use super::*;

impl ContractValidator<'_, '_> {
    pub(super) fn validate_callable_specs(&mut self) {
        for fact in &self.types.facts.callable_spec_satisfactions {
            if !self.item_is_reachable(fact.item) {
                continue;
            }
            let Some(bound) = &fact.effect_bound else {
                continue;
            };
            let Some(summary) = self.effects.facts.item_effects.get(&fact.item).cloned() else {
                self.reject_item(
                    fact.item,
                    fact.span,
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "callable spec validation requires the implementation's solved effect summary",
                );
                continue;
            };
            let bound = match lower_checked_row(
                self.types,
                self.registry,
                bound,
                fact.span,
                self.effects,
            ) {
                Ok(bound) => bound,
                Err(error) => {
                    self.reject_item_with_reason(
                        fact.item,
                        fact.span,
                        error.code,
                        error.message,
                        error.reason,
                    );
                    continue;
                }
            };
            let coverage = crate::EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            if !coverage.row_covers(&declared_public_boundary(&bound), &summary.escaping_effects) {
                self.reject_item(
                    fact.item,
                    fact.span,
                    EffectDiagnosticCode::EffectOutsideDeclaredRow,
                    "implementation escaping effects do not satisfy the callable spec bound",
                );
            }
        }
    }
}
