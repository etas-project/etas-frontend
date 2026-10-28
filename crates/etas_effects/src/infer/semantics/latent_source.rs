use super::{engine::EffectSemantics, shared::*};

mod structural;

impl EffectSemantics<'_> {
    pub(crate) fn record_callable_assignment(
        &self,
        state: &mut EffectState,
        target: HirExprId,
        value: HirExprId,
    ) -> Result<(), &'static str> {
        let mut root = target;
        let mut whole_binding = true;
        let mut visited = std::collections::HashSet::new();
        let symbol = loop {
            if !visited.insert(root) {
                return Err("callable assignment contains a cyclic place");
            }
            match self.hir.exprs.get(root) {
                Some(HirExpr::Field { base, .. } | HirExpr::Index { base, .. }) => {
                    root = *base;
                    whole_binding = false;
                }
                Some(HirExpr::Path(path)) => {
                    break match &path.resolution {
                        ResolveResult::Resolved(symbol) => *symbol,
                        ResolveResult::PartiallyResolved(partial) => {
                            whole_binding = false;
                            partial
                                .resolved_prefix
                                .ok_or("callable assignment has no resolved place root")?
                        }
                        _ => return Err("callable assignment has no resolved place root"),
                    };
                }
                _ => return Err("callable assignment has no checked place root"),
            }
        };
        let sources = self
            .latent_sources_for_expr(state, value)
            .unwrap_or_else(|| vec![LatentSource::Unavailable { expr: value }]);
        if whole_binding {
            state
                .local_latent_values
                .set_symbol_sources(symbol, sources);
        } else {
            state
                .local_latent_values
                .record_symbol_sources(symbol, sources);
        }
        Ok(())
    }

    pub(crate) fn latent_sources_for_symbol(
        &self,
        state: &EffectState,
        mut symbol: etas_hir::SymbolId,
    ) -> Option<Vec<LatentSource>> {
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(symbol) {
                return None;
            }
            if let Some(sources) = state.local_latent_values.symbol_sources(symbol) {
                return Some(sources.iter().copied().collect());
            }
            let known_global = self.global_values.symbol_sources(symbol).is_some()
                || self
                    .discovered_global_values
                    .symbol_sources(symbol)
                    .is_some();
            let globals = self
                .global_values
                .symbol_sources(symbol)
                .into_iter()
                .flatten()
                .chain(
                    self.discovered_global_values
                        .symbol_sources(symbol)
                        .into_iter()
                        .flatten(),
                )
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            if known_global {
                return Some(globals.into_iter().collect());
            }
            if let Some(target) = self.hir.source_import_targets.get(&symbol) {
                symbol = *target;
                continue;
            }
            let definition = self.hir.symbols.get(symbol)?;
            if self.checked_enum_constructor(symbol) {
                return Some(vec![LatentSource::EnumConstructor(symbol)]);
            }
            return match definition.def {
                SymbolDef::Param { owner, .. }
                    if self.checked_function_parameter(symbol).is_some() =>
                {
                    Some(vec![LatentSource::Parameter { owner, symbol }])
                }
                SymbolDef::Item { item } => match self.hir.items.get(item)? {
                    etas_hir::HirItem::Agent(_) => match self.agent_default_call_target(item) {
                        CallTarget::Direct(unit) => Some(vec![LatentSource::Unit(unit)]),
                        _ => None,
                    },
                    etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_) => {
                        Some(vec![LatentSource::Unit(EffectUnit::Item(item))])
                    }
                    _ => self
                        .top_level_anonymous_flow_unit(item)
                        .map(|unit| vec![LatentSource::Unit(unit)]),
                },
                SymbolDef::TopLevelLet { item, .. } => self
                    .top_level_anonymous_flow_unit(item)
                    .map(|unit| vec![LatentSource::Unit(unit)]),
                _ => None,
            };
        }
    }

    pub(crate) fn checked_function_parameter(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Option<&etas_types::FlowType> {
        let definition = self.hir.symbols.get(symbol)?;
        if !matches!(definition.def, SymbolDef::Param { .. }) {
            return None;
        }
        let Type::Function(flow) = self.types.store.get(self.symbol_value_type(symbol)?)? else {
            return None;
        };
        if flow.effects.is_none() && !self.parameter_effects.contains_key(&symbol) {
            return None;
        }
        Some(flow)
    }

    pub(crate) fn latent_source_summary(
        &self,
        source: LatentSource,
        span: Span,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        match source {
            LatentSource::Reference { .. } | LatentSource::StdReference { .. } => {
                self.reference_summary(source, span)
            }
            LatentSource::Captured { .. } | LatentSource::Metadata(_) => {
                self.solve_callable_summary(source, span)
            }
            _ => self.latent_leaf_summary(source, span).ok_or_else(|| {
                super::specialize::EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "callable source requires a solved checked effect fact".into(),
                }
            }),
        }
    }

    pub(super) fn latent_leaf_summary(
        &self,
        source: LatentSource,
        span: Span,
    ) -> Option<EffectSummary> {
        match source {
            LatentSource::Captured { .. } | LatentSource::Metadata(_) => None,
            LatentSource::Unavailable { .. } => None,
            LatentSource::EnumConstructor(symbol) => self
                .checked_enum_constructor(symbol)
                .then(EffectSummary::local),
            LatentSource::Reference { .. } | LatentSource::StdReference { .. } => {
                self.reference_summary(source, span).ok()
            }
            LatentSource::Unit(EffectUnit::Item(item)) if self.frozen_calls.contains_key(&item) => {
                match self.frozen_calls.get(&item)? {
                    crate::FrozenCallableSummary::Solved(summary) => Some((**summary).clone()),
                    crate::FrozenCallableSummary::AnalyzePinnedSource => self
                        .inputs
                        .unit_effects
                        .get(&EffectUnit::Item(item))
                        .cloned(),
                    crate::FrozenCallableSummary::Unavailable => None,
                }
            }
            LatentSource::Unit(unit) => self.inputs.unit_effects.get(&unit).cloned(),
            LatentSource::Parameter { owner, symbol } => {
                let data = self.hir.symbols.get(symbol)?;
                if !matches!(data.def, SymbolDef::Param { owner: actual, .. } if actual == owner) {
                    return None;
                }
                let flow = self.checked_function_parameter(symbol)?;
                let Some(row) = &flow.effects else {
                    return self.parameter_effects.get(&symbol).cloned();
                };
                let mut summary =
                    self.summary_for_invoked_flow_row(self.row_from_type_ref(row), span);
                // A declared escaping row never describes internal requested or
                // handled actions. Keep the call symbolic until the argument is known.
                summary.action_trace = ActionTraceDomain::ParameterCall {
                    target: crate::ParameterCallTarget::Native(symbol),
                    parameter: data.name.clone(),
                    span,
                };
                summary.handler_requests = crate::RequestInstantiationSummary::parameter(
                    crate::ParameterCallTarget::Native(symbol),
                    data.name.clone(),
                    span,
                );
                Some(summary)
            }
        }
    }
}

#[cfg(test)]
mod tests;
