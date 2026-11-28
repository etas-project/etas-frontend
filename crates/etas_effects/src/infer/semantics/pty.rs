use etas_hir::{HirArg, HirExpr, HirExprId, ResolveResult, SymbolDef};
use etas_types::{
    EffectArgRef, Type,
    method::{MethodEvidence, MethodTarget},
};

use super::{engine::EffectSemantics, specialize::EffectSpecializationError};
use crate::{ActionRef, Effect, EffectSummary, infer::pty};

impl EffectSemantics<'_> {
    pub(super) fn pty_action(&self, action: &ActionRef) -> Option<&'static str> {
        let signature = self.registry.action_signature(action)?;
        let path = match signature.name.as_str() {
            "start_pty" => pty::START,
            "read_pty" => "std.effects.Command.read_pty",
            "write_pty" => "std.effects.Command.write_pty",
            "resize_pty" => "std.effects.Command.resize_pty",
            "stop_pty" => "std.effects.Command.stop_pty",
            _ => return None,
        };
        (self.registry.action_by_name(path).as_ref() == Some(action)).then_some(path)
    }

    pub(super) fn select_performed_pty_profile(
        &self,
        expr: HirExprId,
        effect: &mut Effect,
        payload: &[HirArg],
    ) -> Result<(), &'static str> {
        let Effect::AppliedAction(instance) = effect else {
            return Ok(());
        };
        let Some(action) = self.pty_action(&instance.action) else {
            return Ok(());
        };
        let [selector] = &mut instance.args[..] else {
            return Err("terminal action requires one sandbox profile selector");
        };
        let (index, name) = if action == pty::START {
            (1, "sandbox")
        } else {
            (0, "handle")
        };
        let input = pty::payload_argument(payload, index, name)
            .ok_or("terminal action is missing its checked profile or handle")?;
        let actual = self.pty_origin(input);
        if let Some(expected) =
            pty::checked_pty_profile_constraint(self.hir, self.std_registry, expr)?
            && actual != EffectArgRef::Wildcard
            && actual != EffectArgRef::Path(expected)
        {
            return Err("terminal action selector differs from its originating sandbox profile");
        }
        *selector = actual;
        Ok(())
    }

    fn pty_origin(&self, expr: HirExprId) -> EffectArgRef {
        if let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr)
            && let ResolveResult::Resolved(symbol) = path.resolution
        {
            if matches!(
                self.hir.symbols.get(symbol).map(|s| &s.def),
                Some(SymbolDef::Param { .. })
            ) {
                return EffectArgRef::Wildcard;
            }
            if self
                .types
                .facts
                .expression_producer_type(expr)
                .and_then(|ty| self.types.store.get(ty))
                .is_some_and(|ty| matches!(ty, Type::Nominal(t) if t.name == pty::PROFILE))
                && let Some(path) = pty::static_profile_path(self.hir, self.std_registry, symbol)
            {
                return EffectArgRef::Path(path);
            }
        }
        self.memory_provenance
            .pty_profile(expr)
            .unwrap_or(EffectArgRef::Wildcard)
    }

    /// A separately checked wrapper has a wildcard in its body. Narrow every
    /// occurrence of that request, including traces, handler matching and host rights.
    pub(super) fn specialize_pty_call(
        &self,
        call: HirExprId,
        summary: EffectSummary,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let Some((action, name)) = summary.requested_actions.effects.iter().find_map(|effect| {
            let Effect::AppliedAction(instance) = effect else {
                return None;
            };
            if instance.args != [EffectArgRef::Wildcard] {
                return None;
            }
            Some((instance.action.clone(), self.pty_action(&instance.action)?))
        }) else {
            return Ok(summary);
        };
        let input = if name == pty::START {
            pty::start_profile(self.hir, self.types, self.std_registry, call)
        } else {
            self.pty_method_receiver(call, name)
        };
        let Some(input) = input else {
            return Ok(summary);
        };
        let origin = self.pty_origin(input);
        if origin == EffectArgRef::Wildcard {
            return Ok(summary);
        }
        self.specialize_summary_at_selector(&summary, action.tag, Some(action.action), &origin)
    }

    fn pty_method_receiver(&self, call: HirExprId, action: &str) -> Option<HirExprId> {
        let fact = self.types.facts.checked_method_calls.get(&call)?;
        if !matches!(self.types.store.get(fact.receiver_type)?,Type::Nominal(t) if t.name == pty::HANDLE)
        {
            return None;
        }
        let HirExpr::MethodCall {
            receiver, method, ..
        } = self.hir.exprs.get(call)?
        else {
            return None;
        };
        if let MethodTarget::External { identity } = &fact.target {
            let key = identity.implementation().subject();
            if key.module() != ["std", "host", "command"]
                || key.declaration() != ["PtyHandle"]
                || identity.implementation().selected_spec().is_some()
            {
                return None;
            }
        } else if let MethodTarget::Source { symbol, .. } = fact.target {
            let module = self.hir.symbols.get(symbol)?.defining_module;
            let name = self.hir.modules_arena.get(module)?.name.as_ref()?;
            if !matches!(fact.evidence, MethodEvidence::Inherent)
                || !name
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .eq(["std", "host", "command"])
            {
                return None;
            }
        } else {
            return None;
        }
        (action
            .strip_prefix("std.effects.Command.")?
            .strip_suffix("_pty")?
            == method)
            .then_some(*receiver)
    }
}
