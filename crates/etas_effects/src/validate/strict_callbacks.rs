use std::collections::BTreeSet;

use super::*;

impl ContractValidator<'_, '_> {
    /// Hash and Order have checked behavioral contracts, not a consequence of an empty
    /// escaping row. Storage adapters execute inside the same pure boundary.
    pub(super) fn validate_strict_callbacks(
        &mut self,
        imported: &[crate::AnchoredExternalMetadata<crate::ExternalSpecImplementationMetadata>],
    ) {
        let specs = etas_types::SpecFacts::from(self.types.facts.as_ref());
        let mut source = BTreeSet::new();
        let mut external = BTreeSet::new();
        let pure_methods = self.types.facts.methods.entries().filter(|method| {
            method.spec_member.as_ref().is_some_and(|member| {
                etas_types::data_equality::is_pure_collection_member(&specs, member)
            })
        });
        for method in
            pure_methods.chain(self.types.facts.storage_methods().map(|(method, _)| method))
        {
            match &method.target {
                etas_types::method::MethodTarget::Source { callable, .. } => {
                    source.insert(*callable);
                }
                etas_types::method::MethodTarget::External { identity } => {
                    external.insert(identity.clone());
                }
                _ => {}
            }
        }
        for item in source {
            if !self.item_is_reachable(item) {
                continue;
            }
            let summary = self.effects.facts.item_effects.get(&item);
            let suspension = self
                .effects
                .facts
                .suspension
                .get(&crate::EffectUnit::Item(item));
            let pure = summary.is_some_and(|summary| {
                summary.determinism == crate::Determinism::Deterministic
                    && summary.independent_determinism == Some(crate::Determinism::Deterministic)
                    && summary.escaping_effects.effects.is_empty()
                    && summary.escaping_effects.open.is_none()
                    && summary.requested_actions.effects.is_empty()
                    && summary.requested_actions.open.is_none()
                    && summary.handler_requests == crate::RequestInstantiationSummary::default()
                    && !matches!(summary.support, crate::InterpreterSupport::Rejected(_))
            }) && suspension.is_some_and(|summary| summary.proven_non_suspending());
            if !pure {
                self.reject_item(
                    item,
                    self.hir.items[item].span(),
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    "Hash implementations, Order comparators and storage adapters require deterministic, action-free, non-suspending execution; ![] alone is insufficient",
                );
            }
        }
        // Signature tables include unimported overload candidates. Validate
        // the implementation bodies selected by the dependency loader; actual
        // calls still require their complete checked method graph.
        for implementation in imported
            .iter()
            .filter(|method| external.contains(&method.metadata.identity))
        {
            let identity = &implementation.metadata.identity;
            let method = self
                .types
                .facts
                .external_methods
                .iter()
                .find(|method| &method.identity == identity);
            let body = implementation.metadata.checked_body();
            let pure = body.is_some_and(|body| {
                use etas_package_metadata::CallableValueDeterminism as D;
                let empty = |row: &etas_package_metadata::EffectRow| {
                    row.effects.is_empty() && row.tail.is_none()
                };
                body.behavior.determinism == D::Deterministic
                    && body.behavior.independent_determinism == Some(D::Deterministic)
                    && body
                        .behavior
                        .suspension
                        .is_some_and(|summary| !summary.may_suspend && !summary.unknown)
                    && empty(&body.public_effects)
                    && empty(&body.requested_actions)
                    && empty(&body.handled_requested_actions)
                    && body.handler_requests.as_ref().is_some_and(|requests| {
                        requests.actions.is_empty() && requests.parameters.is_empty()
                    })
            });
            if !pure && let Some(method) = method {
                self.effects.diagnostics.push(etas_core::Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    method.span,
                    "imported Hash implementations, Order comparators and storage adapters require checked deterministic, action-free, non-suspending behavior",
                ));
            }
        }
    }
}
