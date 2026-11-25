//! Origins of requested behavior in a confined local Agent instance.
use etas_hir::{
    HirExpr, HirExprId, HirFieldInit, HirItemId, HirOwner, HirPat, HirStmt, ResolveResult,
    SymbolDef, SymbolId,
};
use etas_types::{Type, TypeId, facts::methods::MethodCallKind, method::MethodTarget};

use super::{
    engine::EffectSemantics,
    specialize::EffectSpecializationError,
    state::{EffectState, LatentSource},
};
use crate::{EffectSummary, EffectUnit};

impl EffectSemantics<'_> {
    fn alias_binding(&self, owner: HirOwner, initializer: HirExprId) -> Option<SymbolId> {
        let HirOwner::Stmt(stmt) = owner else {
            return None;
        };
        let HirStmt::Let { pat, value, .. } = self.hir.stmts.get(stmt)? else {
            return None;
        };
        let HirPat::Binding { symbol, .. } = self.hir.pats.get(*pat)? else {
            return None;
        };
        (*value == initializer).then_some(*symbol)
    }

    fn resolved_value_symbol(&self, expr: HirExprId) -> Option<SymbolId> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        if let ResolveResult::Resolved(symbol) = path.resolution {
            Some(symbol)
        } else {
            None
        }
    }

    pub(super) fn agent_allocation(&self, mut symbol: SymbolId) -> Option<HirExprId> {
        if !self
            .symbol_value_type(symbol)
            .is_some_and(|ty| self.types.facts.is_active_agent(&self.types.store, ty))
        {
            return None;
        }
        let mut visited = std::collections::BTreeSet::new();
        while visited.insert(symbol) {
            let SymbolDef::Local {
                binding,
                initializer: Some(value),
                ..
            } = self.hir.symbols.get(symbol)?.def
            else {
                return None;
            };
            if self.alias_binding(HirOwner::Stmt(binding), value) != Some(symbol) {
                return None;
            }
            if matches!(self.hir.exprs.get(value), Some(HirExpr::Record(_))) {
                return Some(value);
            }
            symbol = self.resolved_value_symbol(value)?;
        }
        None
    }

    fn confined_uses(
        &self,
        item: HirItemId,
        has_symbol: impl Fn(SymbolId) -> bool,
        accepts: impl Fn(HirExprId, Option<&str>, HirOwner) -> bool,
    ) -> bool {
        let view = self.context.view();
        let index = view.index();
        for (expr, value) in self.hir.exprs.iter() {
            if index.expr_item.get(&expr) != Some(&item) {
                continue;
            }
            match value {
                HirExpr::Path(path) => {
                    let (symbol, field) = match &path.resolution {
                        ResolveResult::Resolved(symbol) => (Some(*symbol), None),
                        ResolveResult::PartiallyResolved(path) => (
                            path.resolved_prefix,
                            path.remaining.first().map(String::as_str),
                        ),
                        _ => continue,
                    };
                    if !symbol.is_some_and(&has_symbol) {
                        continue;
                    }
                    if !MethodCallKind::same_callable_body(&view, expr, item)
                        || !index
                            .expr_owner
                            .get(&expr)
                            .is_some_and(|owner| accepts(expr, field, *owner))
                    {
                        return false;
                    }
                }
                HirExpr::Record(record)
                    if record.fields.iter().any(|field| {
                        matches!(field, HirFieldInit::Shorthand {
                        resolution: ResolveResult::Resolved(symbol), ..
                    } if has_symbol(*symbol))
                    }) =>
                {
                    return false;
                }
                _ => {}
            }
        }
        true
    }

    fn confined_constructor(&self, allocation: HirExprId, item: HirItemId) -> bool {
        if let Some(closed) = self
            .type_queries
            .confined_agent_constructions
            .borrow()
            .get(&allocation)
        {
            return *closed;
        }
        let view = self.context.view();
        let index = view.index();
        let binding = index
            .expr_owner
            .get(&allocation)
            .and_then(|owner| self.alias_binding(*owner, allocation));
        let closed = binding
            .is_some_and(|binding| self.agent_allocation(binding) == Some(allocation))
            && MethodCallKind::same_callable_body(&view, allocation, item)
            && self.confined_uses(
                item,
                |symbol| self.agent_allocation(symbol) == Some(allocation),
                |expr, field, owner| {
                    if field.is_none()
                        && self
                            .alias_binding(owner, expr)
                            .is_some_and(|alias| self.agent_allocation(alias) == Some(allocation))
                    {
                        return true;
                    }
                    let HirOwner::Expr(call) = owner else {
                        return false;
                    };
                    let Some(method) = self.types.facts.checked_method_calls.get(&call) else {
                        return false;
                    };
                    if method.kind != MethodCallKind::AgentRequest
                        || !matches!(
                            method.target,
                            MethodTarget::Source { .. } | MethodTarget::Agent { .. }
                        )
                    {
                        return false;
                    }
                    self.checked_method_argument_slots(call)
                        .ok()
                        .is_some_and(|slots| {
                            method
                                .receiver
                                .value_slot()
                                .and_then(|slot| slots.get(slot as usize).copied().flatten())
                                == Some(expr)
                        })
                },
            );
        self.type_queries
            .confined_agent_constructions
            .borrow_mut()
            .insert(allocation, closed);
        closed
    }

    fn confined_agent_methods(&self, ty: TypeId) -> bool {
        if let Some(closed) = self.type_queries.confined_agent_methods.borrow().get(&ty) {
            return *closed;
        }
        let store = &self.types.store;
        let facts = &self.types.facts;
        let component = match store.get(ty) {
            Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
            _ => ty,
        };
        let closed = !etas_types::active_agent::has_main(facts, store, ty)
            && facts
                .agent_methods
                .values()
                .filter(|method| method.component == component)
                .all(|method| {
                    let Some((receiver, layout)) = self.agent_receiver_parameter(method.body)
                    else {
                        return false;
                    };
                    let Some(Type::Record(layout)) = store.get(layout) else {
                        return false;
                    };
                    let field_exists =
                        |name: &str| layout.fields.iter().any(|field| field.name == name);
                    self.confined_uses(
                        method.body,
                        |symbol| symbol == receiver,
                        |expr, field, owner| {
                            if field.is_some_and(field_exists) {
                                return true;
                            }
                            let HirOwner::Expr(parent) = owner else {
                                return false;
                            };
                            match self.hir.exprs.get(parent) {
                                Some(HirExpr::Field { base, field, .. }) if *base == expr => {
                                    field_exists(field)
                                }
                                Some(HirExpr::MethodCall { receiver, .. }) if *receiver == expr => {
                                    facts.checked_method_calls.get(&parent).is_some_and(|call| {
                                        call.kind == MethodCallKind::AgentTurnHelper
                                    })
                                }
                                _ => false,
                            }
                        },
                    )
                });
        self.type_queries
            .confined_agent_methods
            .borrow_mut()
            .insert(ty, closed);
        closed
    }

    pub(super) fn record_confined_agent_state(
        &self,
        allocation: HirExprId,
        state: &mut EffectState,
    ) {
        if !matches!(self.hir.exprs.get(allocation), Some(HirExpr::Record(_))) {
            return;
        }
        let Some((ty, item)) = self
            .types
            .facts
            .expression_producer_type(allocation)
            .zip(state.owner)
        else {
            return;
        };
        let Some(paths) = self
            .types
            .facts
            .agent_state_type(&self.types.store, ty)
            .ok()
            .flatten()
            .and_then(|layout| self.value_payload_paths(layout))
        else {
            return;
        };
        if paths.is_empty()
            || !self.confined_constructor(allocation, item)
            || !self.confined_agent_methods(ty)
        {
            return;
        }
        for path in paths {
            let origins = self
                .projected_sources(allocation, &path, state)
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: allocation }].into());
            state.agent_state.insert((allocation, path), origins);
        }
    }

    pub(super) fn apply_committed_agent_state(
        &self,
        site: &etas_hir_analysis::interprocedural::CallSite<EffectUnit>,
        callee: EffectUnit,
        summary: &EffectSummary,
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        let Some(method) = self.types.facts.checked_method_calls.get(&site.call) else {
            return Ok(());
        };
        if method.kind != MethodCallKind::AgentRequest
            || !method.receiver.is_updating()
            || state.agent_state.is_empty()
            || summary
                .exits
                .as_ref()
                .is_some_and(crate::CallableExitSummary::has_only_terminal_exits)
        {
            return Ok(());
        }
        let receiver = method
            .receiver
            .value_slot()
            .expect("updating receiver slot");
        let slots = self.checked_method_argument_slots(site.call)?;
        let Some(allocation) = slots[receiver as usize]
            .and_then(|expr| self.resolved_value_symbol(expr))
            .and_then(|symbol| self.agent_allocation(symbol))
        else {
            return Ok(());
        };
        let paths = state
            .agent_state
            .keys()
            .filter_map(|(owner, path)| (*owner == allocation).then_some(path.clone()))
            .collect::<Vec<_>>();
        let updates = self.agent_state_at_return(site, callee, summary, state, paths)?;
        for (path, origins) in updates {
            state.agent_state.insert((allocation, path), origins);
        }
        Ok(())
    }
}
