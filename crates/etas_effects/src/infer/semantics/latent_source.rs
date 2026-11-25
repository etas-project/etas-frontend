use super::{engine::EffectSemantics, shared::*};

mod structural;

impl EffectSemantics<'_> {
    pub(crate) fn record_callable_assignment(
        &self,
        state: &mut EffectState,
        target: HirExprId,
        value: HirExprId,
    ) -> Result<(), &'static str> {
        use etas_code::descriptor::ValueProjectionStep as Step;
        let mut root = target;
        let mut written = Vec::new();
        let mut indexed = false;
        let mut visited = std::collections::HashSet::new();
        let symbol = loop {
            if !visited.insert(root) {
                return Err("callable assignment contains a cyclic place");
            }
            match self.hir.exprs.get(root) {
                Some(HirExpr::Field { base, field, .. }) => {
                    root = *base;
                    written.push(Step::Field(field.clone()));
                }
                Some(HirExpr::Index { base, .. }) => {
                    root = *base;
                    indexed = true;
                    let kind = self
                        .types
                        .facts
                        .expr_types
                        .get(base)
                        .and_then(|ty| self.types.store.get(*ty));
                    written.push(if matches!(kind, Some(Type::Map { .. })) {
                        Step::MapValue
                    } else {
                        Step::SequenceElement
                    });
                }
                Some(HirExpr::Path(path)) => {
                    break match &path.resolution {
                        ResolveResult::Resolved(symbol) => *symbol,
                        ResolveResult::PartiallyResolved(partial) => {
                            written
                                .extend(partial.remaining.iter().rev().cloned().map(Step::Field));
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
        if written.is_empty() {
            state
                .local_latent_values
                .set_symbol_sources(symbol, sources);
            self.record_symbol_projections(state, symbol, value, &[]);
        } else {
            written.reverse();
            // A field assignment can replace a previously checked callable
            // origin without replacing the enclosing record.
            let updates = state
                .local_latent_values
                .symbol_projections
                .keys()
                .filter_map(|(owner, path)| {
                    if *owner != symbol {
                        return None;
                    }
                    let tail = path.strip_prefix(written.as_slice())?;
                    let updated = self
                        .projected_sources(value, tail, state)
                        .unwrap_or_else(|| {
                            [LatentSource::Unavailable { expr: value }]
                                .into_iter()
                                .collect()
                        });
                    Some(((*owner, path.clone()), updated))
                })
                .collect::<Vec<_>>();
            for (position, updated) in updates {
                let origins = state
                    .local_latent_values
                    .symbol_projections
                    .entry(position)
                    .or_default();
                if !indexed {
                    origins.clear();
                }
                origins.extend(updated);
            }
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
                    if self.symbol_type_contains_flow(symbol)
                        || self
                            .symbol_value_type(symbol)
                            .is_some_and(|ty| self.type_has_parameters(ty))
                        || self.checked_callable_contract_parameter(symbol).is_some()
                        || self.symbolic_callable_parameter(symbol)
                        || !self.generic_parameter_payloads(symbol).is_empty()
                        || self
                            .symbol_value_type(symbol)
                            .is_some_and(|ty| self.descriptor_contract_type(ty).is_some()) =>
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

    pub(crate) fn checked_callable_contract_parameter(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Option<&etas_types::FlowType> {
        let definition = self.hir.symbols.get(symbol)?;
        if !matches!(definition.def, SymbolDef::Param { .. }) {
            return None;
        }
        let parameter = self.symbol_value_type(symbol)?;
        let contract = self
            .descriptor_contract_type(parameter)
            .unwrap_or(parameter);
        let flow = self.checked_payload_callable_contract(contract)?;
        if flow.declared_effects().is_none() && !self.parameter_effects.contains_key(&symbol) {
            return None;
        }
        Some(flow)
    }

    fn checked_payload_callable_contract(&self, ty: TypeId) -> Option<&etas_types::FlowType> {
        let mut pending = vec![ty];
        let mut visited = std::collections::BTreeSet::new();
        let mut contract = None;
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            // Generic data fields do not grant the generic body an invocation
            // contract. Their origins still travel with the actual argument.
            // Only checked callable payloads constrain this shared contract;
            // an unrelated data subtree must not hide a record's callback.
            if !self.type_contains_flow(ty) {
                continue;
            }
            match self.types.store.get(ty)? {
                Type::Function(flow) => match contract {
                    None => contract = Some(flow),
                    Some(existing) if existing == flow => {}
                    Some(_) => return None,
                },
                Type::Nominal(_) | Type::Applied { .. } => {
                    pending.push(self.applied_representation(ty).ok()??);
                }
                Type::Record(record) => pending.extend(record.fields.iter().map(|field| field.ty)),
                Type::Tuple(elements) => pending.extend(elements),
                Type::Array(inner)
                | Type::List(inner)
                | Type::Set(inner)
                | Type::Slice(inner)
                | Type::Option(inner) => pending.push(*inner),
                Type::Map { key, value }
                | Type::Result {
                    ok: key,
                    err: value,
                } => pending.extend([*key, *value]),
                _ if self.type_contains_flow(ty) || self.type_has_parameters(ty) => return None,
                _ => {}
            }
        }
        // Aggregate origins already contain the union of their callbacks. They
        // may share a symbolic invocation contract only when every callable
        // payload has the same checked arrow type. The concrete argument still
        // supplies requested actions, handled actions, trace and completion.
        contract
    }

    pub(crate) fn latent_source_summary(
        &self,
        source: LatentSource,
        span: Span,
    ) -> Result<EffectSummary, super::specialize::EffectSpecializationError> {
        match source {
            LatentSource::FrozenContract { .. }
            | LatentSource::ExternalFrozen { .. }
            | LatentSource::ExternalReference { .. } => self.solve_callable_summary(source, span),
            LatentSource::Reference { .. } | LatentSource::StdReference { .. } => {
                self.reference_summary(source, span)
            }
            LatentSource::Captured { .. }
            | LatentSource::Metadata(_)
            | LatentSource::SpecMethod { .. }
            | LatentSource::MetadataSpecMethod { .. } => self.solve_callable_summary(source, span),
            _ => self.latent_leaf_summary(source, span).ok_or_else(|| {
                super::specialize::EffectSpecializationError::InvalidCallableEnvironment {
                    reason: format!(
                        "callable source {source:?} requires a solved checked effect fact"
                    ),
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
            LatentSource::DerivedResponseDecoder(arm) => self.inference_decoder_summary(arm),
            LatentSource::Projection(id) => {
                let projection = self.parameter_projection(id)?;
                let flow = self.checked_payload_callable_contract(projection.ty)?;
                let row = flow.declared_effects()?;
                let name = self.hir.symbols.get(projection.parameter)?.name.clone();
                let mut summary =
                    self.summary_for_invoked_flow_row(self.row_from_type_ref(row), span);
                summary.independent_determinism = Some(crate::Determinism::Deterministic);
                summary.requested_actions = crate::EffectRow::empty();
                summary.action_trace = ActionTraceDomain::ParameterCall {
                    target: crate::ParameterCallTarget::Projection(id),
                    parameter: name.clone(),
                    span,
                };
                summary.handler_requests = crate::RequestInstantiationSummary::parameter(
                    crate::ParameterCallTarget::Projection(id),
                    name,
                    span,
                );
                Some(summary)
            }
            LatentSource::FrozenContract { .. }
            | LatentSource::ExternalFrozen { .. }
            | LatentSource::ExternalReference { .. } => None,
            LatentSource::Captured { .. }
            | LatentSource::Metadata(_)
            | LatentSource::SpecMethod { .. }
            | LatentSource::MetadataSpecMethod { .. } => None,
            LatentSource::Unavailable { .. }
            | LatentSource::Existential { .. }
            | LatentSource::MetadataObject(_) => None,
            LatentSource::EnumConstructor(symbol) => self
                .checked_enum_constructor(symbol)
                .then(EffectSummary::local),
            LatentSource::Reference { .. } | LatentSource::StdReference { .. } => {
                self.reference_summary(source, span).ok()
            }
            LatentSource::Unit(EffectUnit::Item(item)) if self.frozen_calls.contains_key(&item) => {
                match self.frozen_calls.get(&item)? {
                    crate::FrozenCallableSummary::Solved(summary)
                    | crate::FrozenCallableSummary::Replaced { pinned: summary } => {
                        Some((**summary).clone())
                    }
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
                let flow = self.checked_callable_contract_parameter(symbol)?;
                let Some(row) = flow.declared_effects() else {
                    return self.parameter_effects.get(&symbol).cloned();
                };
                let mut summary =
                    self.summary_for_invoked_flow_row(self.row_from_type_ref(row), span);
                // The row describes the callback's possible effects, not work
                // performed independently by its caller.
                summary.independent_determinism = Some(crate::Determinism::Deterministic);
                // A declared escaping row never describes internal requested or
                // handled actions. Keep the call symbolic until the argument is known.
                summary.requested_actions = crate::EffectRow::empty();
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
