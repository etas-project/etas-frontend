use etas_hir::{HirExpr, HirExprId, HirItem, ResolveResult, SymbolDef, SymbolKind};
use etas_hir_analysis::interprocedural::{CallTarget, UnitContext};

use super::engine::EffectSemantics;
use super::state::EffectState;
use crate::infer::unit::EffectUnit;

impl EffectSemantics<'_> {
    pub(crate) fn call_target_for(
        &mut self,
        context: UnitContext<EffectUnit>,
        call: HirExprId,
        callee: HirExprId,
        state: &EffectState,
    ) -> CallTarget<EffectUnit> {
        let target = self.source_call_target_for(context, call, callee, state);
        if let CallTarget::Direct(unit) = target {
            self.inputs
                .native_call_targets
                .entry(call)
                .or_default()
                .insert(unit);
        }
        if matches!(target, CallTarget::Dynamic)
            && let Some(sources) = self.latent_sources_for_expr(state, callee)
        {
            for source in sources {
                match source {
                    super::state::LatentSource::Unit(unit)
                    | super::state::LatentSource::Reference { unit, .. }
                    | super::state::LatentSource::Captured { unit, .. } => {
                        self.inputs
                            .native_call_targets
                            .entry(call)
                            .or_default()
                            .insert(unit);
                    }
                    _ => {}
                }
            }
        }
        if let CallTarget::Direct(EffectUnit::Item(item)) = target
            && self
                .frozen_calls
                .get(&item)
                .is_some_and(crate::FrozenCallableSummary::uses_snapshot)
        {
            self.frozen_call_targets.insert(call, item);
            return CallTarget::External;
        }
        target
    }

    fn source_call_target_for(
        &mut self,
        context: UnitContext<EffectUnit>,
        call: HirExprId,
        callee: HirExprId,
        state: &EffectState,
    ) -> CallTarget<EffectUnit> {
        if let Some(symbol) = self.types.facts.agent_method_calls.get(&call) {
            if let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) {
                let resolved = match &path.resolution {
                    ResolveResult::Resolved(symbol) => {
                        self.types.facts.symbol_types.contains_key(symbol)
                    }
                    ResolveResult::PartiallyResolved(partial) => partial
                        .resolved_prefix
                        .is_some_and(|symbol| self.types.facts.symbol_types.contains_key(&symbol)),
                    _ => false,
                };
                if !resolved {
                    return CallTarget::Incomplete;
                }
            }
            return self
                .types
                .facts
                .agent_methods
                .get(symbol)
                .map(|method| CallTarget::Direct(EffectUnit::Item(method.body)))
                .unwrap_or(CallTarget::Incomplete);
        }
        let Some(callee_expr) = self.hir.exprs.get(callee) else {
            return CallTarget::Incomplete;
        };
        let HirExpr::Path(path) = callee_expr else {
            return if self.is_typed_flow_expr(callee) {
                CallTarget::Dynamic
            } else {
                CallTarget::External
            };
        };

        let ResolveResult::Resolved(symbol) = path.resolution else {
            return CallTarget::Incomplete;
        };
        let Some(symbol_data) = self.hir.symbols.get(symbol) else {
            return CallTarget::Incomplete;
        };
        if self.is_typed_flow_expr(callee)
            && let Some(sources) = state.local_latent_values.symbol_sources(symbol)
            && !sources.is_empty()
        {
            return match sources.first().filter(|_| sources.len() == 1) {
                Some(super::state::LatentSource::Unit(unit)) => CallTarget::Direct(*unit),
                _ => CallTarget::Dynamic,
            };
        }
        match (&symbol_data.kind, &symbol_data.def) {
            (_, SymbolDef::Item { item }) => match self.hir.items.get(*item) {
                Some(HirItem::Agent(_)) => self.agent_default_call_target(*item),
                Some(HirItem::Flow(_)) if self.types.facts.symbol_types.contains_key(&symbol) => {
                    CallTarget::Direct(EffectUnit::Item(*item))
                }
                Some(HirItem::Flow(_)) => CallTarget::Incomplete,
                Some(HirItem::Tool(tool)) => match tool.body {
                    etas_hir::HirToolBody::Source(_)
                        if self.types.facts.symbol_types.contains_key(&symbol) =>
                    {
                        CallTarget::Direct(EffectUnit::Item(*item))
                    }
                    etas_hir::HirToolBody::Source(_) => CallTarget::Incomplete,
                    etas_hir::HirToolBody::Decl { .. } => CallTarget::External,
                    etas_hir::HirToolBody::Error { .. } => CallTarget::Incomplete,
                },
                Some(HirItem::TopLevelLet(_)) => self
                    .top_level_anonymous_flow_unit(*item)
                    .map(CallTarget::Direct)
                    .unwrap_or_else(|| {
                        if self.is_typed_flow_expr(callee) {
                            CallTarget::Direct(EffectUnit::Item(*item))
                        } else {
                            CallTarget::External
                        }
                    }),
                Some(_) => CallTarget::External,
                None => CallTarget::Incomplete,
            },
            (_, SymbolDef::TopLevelLet { item, .. }) => self
                .top_level_anonymous_flow_unit(*item)
                .map(CallTarget::Direct)
                .unwrap_or_else(|| {
                    if self.is_typed_flow_expr(callee) {
                        CallTarget::Direct(EffectUnit::Item(*item))
                    } else {
                        CallTarget::External
                    }
                }),
            (
                _,
                SymbolDef::ImportAlias {
                    path,
                    origin: etas_hir::ImportAliasOrigin::SourceImport,
                },
            ) => {
                if let Some(item) = self.source_item_for_path(path) {
                    self.source_import_target(item)
                } else if path.first().is_some_and(|segment| segment == "std")
                    || self.has_external_summary_for_path(path)
                {
                    CallTarget::External
                } else {
                    CallTarget::Incomplete
                }
            }
            (_, SymbolDef::ImportAlias { .. }) => CallTarget::External,
            (
                SymbolKind::Type | SymbolKind::EnumVariant | SymbolKind::Enum | SymbolKind::Field,
                _,
            ) => CallTarget::External,
            (SymbolKind::Param, _) | (_, SymbolDef::Param { .. })
                if !self.is_typed_flow_expr(callee) =>
            {
                CallTarget::Incomplete
            }
            _ if context.unit.owner().is_some() => {
                if let Some(unit) = context
                    .unit
                    .owner()
                    .and_then(|owner| self.local_anonymous_flow_unit_for_symbol(owner, symbol))
                {
                    CallTarget::Direct(unit)
                } else if self.is_typed_flow_expr(callee) {
                    CallTarget::Dynamic
                } else {
                    CallTarget::External
                }
            }
            _ if self.is_typed_flow_expr(callee) => CallTarget::Dynamic,
            _ => CallTarget::External,
        }
    }
}

impl EffectSemantics<'_> {
    fn source_import_target(&self, item: etas_hir::HirItemId) -> CallTarget<EffectUnit> {
        match self.hir.items.get(item) {
            Some(HirItem::Agent(_)) => self.agent_default_call_target(item),
            Some(HirItem::Flow(_)) => CallTarget::Direct(EffectUnit::Item(item)),
            Some(HirItem::Tool(tool)) => match tool.body {
                etas_hir::HirToolBody::Source(_) => CallTarget::Direct(EffectUnit::Item(item)),
                etas_hir::HirToolBody::Decl { .. } => CallTarget::External,
                etas_hir::HirToolBody::Error { .. } => CallTarget::Incomplete,
            },
            Some(HirItem::TopLevelLet(_)) => self
                .top_level_anonymous_flow_unit(item)
                .map(CallTarget::Direct)
                .unwrap_or_else(|| {
                    if self.item_symbol_has_flow_type(item) {
                        CallTarget::Direct(EffectUnit::Item(item))
                    } else {
                        CallTarget::External
                    }
                }),
            Some(_) => CallTarget::External,
            None => CallTarget::Incomplete,
        }
    }

    pub(crate) fn agent_default_call_target(
        &self,
        item: etas_hir::HirItemId,
    ) -> CallTarget<EffectUnit> {
        let Some(etas_types::ItemSignature::Agent(signature)) =
            self.types.facts.item_signatures.get(&item)
        else {
            return CallTarget::Incomplete;
        };
        let Some(method) = signature
            .default_entry
            .as_ref()
            .and_then(|entry| entry.method)
            .and_then(|method| self.types.facts.agent_methods.get(&method))
        else {
            return CallTarget::Incomplete;
        };
        CallTarget::Direct(EffectUnit::Item(method.body))
    }

    fn item_symbol_has_flow_type(&self, item: etas_hir::HirItemId) -> bool {
        let Some(symbol) = self.item_symbol(item) else {
            return false;
        };
        self.types.facts.symbol_types.contains_key(&symbol)
    }

    fn item_symbol(&self, item: etas_hir::HirItemId) -> Option<etas_hir::SymbolId> {
        match self.hir.items.get(item)? {
            HirItem::Flow(flow) => Some(flow.symbol),
            HirItem::Agent(agent) => Some(agent.symbol),
            HirItem::Tool(tool) => Some(tool.symbol),
            HirItem::TopLevelLet(value) => Some(value.symbol),
            _ => None,
        }
    }
}
