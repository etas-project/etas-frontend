use etas_hir::{HirExpr, HirExprId};
use etas_types::method::MethodTarget;

use super::{engine::EffectSemantics, specialize::EffectSpecializationError};
use crate::infer::unit::EffectUnit;

impl EffectSemantics<'_> {
    /// Source Store bodies share a request across all receiver values. An
    /// application call can narrow that request to its checked resource place.
    pub(super) fn specialize_source_memory_method(
        &self,
        call: HirExprId,
        summary: crate::EffectSummary,
    ) -> Result<crate::EffectSummary, EffectSpecializationError> {
        use crate::Effect;
        use etas_types::EffectArgRef;
        if !summary.requested_actions.effects.iter().any(|effect| {
            matches!(effect, Effect::AppliedAction(action)
                if action.action.tag == crate::MEMORY_TAG && action.args == [EffectArgRef::Wildcard])
        }) {
            return Ok(summary);
        }
        let Some(MethodTarget::External { identity }) = self
            .types
            .facts
            .checked_method_calls
            .get(&call)
            .map(|fact| &fact.target)
        else {
            return Ok(summary);
        };
        let implementation = identity.implementation();
        let subject = self
            .std_registry
            .lookup_qualified(&["std", "memory", "Store"])
            .and_then(|store| self.std_registry.declaration_key(store.id));
        if !subject.is_some_and(|store| {
            implementation.subject() == store && implementation.package() == store.package()
        }) {
            return Ok(summary);
        }
        let Some(HirExpr::MethodCall { receiver, .. } | HirExpr::SpecMethodCall { receiver, .. }) =
            self.hir.exprs.get(call)
        else {
            return Ok(summary);
        };
        let Ok(places) = self.memory_provenance.arguments(*receiver) else {
            return Ok(summary);
        };
        // A formal parameter is not a portable resource identity. Keep its
        // requested row open to all checked receivers until a root is known.
        if !places.iter().all(|place| match place {
            EffectArgRef::Path(path) => self.types.facts.resource_handles.keys().any(|symbol| {
                self.hir
                    .declaration_path(*symbol)
                    .is_some_and(|root| path.starts_with(&root))
            }),
            _ => false,
        }) {
            return Ok(summary);
        }
        let mut alternatives = places.iter().map(|place| {
            self.specialize_summary_at_selector(&summary, crate::MEMORY_TAG, None, place)
        });
        let Some(mut specialized) = alternatives.next().transpose()? else {
            return Ok(summary);
        };
        for alternative in alternatives {
            specialized.join_branch(&alternative?);
        }
        Ok(specialized)
    }

    pub(super) fn checked_method_target(&self, call: HirExprId) -> Option<EffectUnit> {
        match &self.types.facts.checked_method_calls.get(&call)?.target {
            MethodTarget::Source { callable, .. } | MethodTarget::Agent { callable, .. } => {
                Some(EffectUnit::Item(*callable))
            }
            _ => None,
        }
    }

    pub(super) fn checked_method_arguments(
        &self,
        call: HirExprId,
    ) -> Result<Vec<HirExprId>, EffectSpecializationError> {
        self.checked_method_argument_slots(call)?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)
    }

    pub(super) fn checked_method_argument_slots(
        &self,
        call: HirExprId,
    ) -> Result<Vec<Option<HirExprId>>, EffectSpecializationError> {
        let invalid = || EffectSpecializationError::InvalidCallableParameterInterface;
        let fact = self
            .types
            .facts
            .checked_method_calls
            .get(&call)
            .ok_or_else(invalid)?;
        let (receiver, args) = match self.hir.exprs.get(call) {
            Some(
                HirExpr::MethodCall { receiver, args, .. }
                | HirExpr::SpecMethodCall { receiver, args, .. },
            ) => (*receiver, args),
            _ => return Err(invalid()),
        };
        if args.len() != fact.argument_slots.len() {
            return Err(invalid());
        }
        let mut ordered = vec![None; fact.signature.params.len()];
        if let Some(slot) = fact.receiver.value_slot() {
            *ordered.get_mut(slot as usize).ok_or_else(invalid)? = Some(receiver);
        }
        for (arg, slot) in args.iter().zip(&fact.argument_slots) {
            let slot = ordered.get_mut(*slot as usize).ok_or_else(invalid)?;
            if slot.is_some() {
                return Err(invalid());
            }
            *slot = Some(match arg {
                etas_hir::HirArg::Positional(expr)
                | etas_hir::HirArg::Named { value: expr, .. } => *expr,
            });
        }
        for default in &fact.defaults {
            if let etas_types::ParameterDefaultRef::Source { parameter } = default.source {
                *ordered.get_mut(default.slot as usize).ok_or_else(invalid)? = Some(
                    *self
                        .hir
                        .parameter_defaults
                        .get(&parameter)
                        .ok_or_else(invalid)?,
                );
            }
        }
        Ok(ordered)
    }
}
