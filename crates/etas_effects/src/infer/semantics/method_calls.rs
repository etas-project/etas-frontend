use etas_hir::{HirExpr, HirExprId};
use etas_types::method::{MethodReceiver, MethodTarget};

use super::{engine::EffectSemantics, specialize::EffectSpecializationError};
use crate::infer::unit::EffectUnit;

impl EffectSemantics<'_> {
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
        if let MethodReceiver::Value { slot } = fact.receiver {
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
        ordered
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)
    }
}
