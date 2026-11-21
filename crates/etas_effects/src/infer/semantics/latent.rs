use super::engine::EffectSemantics;
use super::shared::*;

impl EffectState {
    pub fn record_anonymous_flow(
        &mut self,
        owner: etas_hir::HirItemId,
        value: HirExprId,
        body: EffectAnonymousFlowBody,
    ) {
        self.local_latent_values
            .record_value(value, EffectUnit::AnonymousFlow { owner, value, body });
    }
}

impl EffectSemantics<'_> {
    pub(crate) fn missing_top_level_flow_value_fact(
        &self,
        unit: EffectUnit,
    ) -> Option<(Span, &'static str)> {
        let EffectUnit::Item(item) = unit else {
            return None;
        };
        let etas_hir::HirItem::TopLevelLet(value) = self.hir.items.get(item)? else {
            return None;
        };
        if !matches!(
            self.hir.exprs.get(value.value?),
            Some(HirExpr::Lambda { .. })
        ) {
            return None;
        }
        if self.types.facts.symbol_types.contains_key(&value.symbol) {
            return None;
        }
        Some((
            value.span,
            "top-level flow value requires checked value type facts",
        ))
    }

    pub(crate) fn latent_sources_for_expr(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> Option<Vec<LatentSource>> {
        if self.types.facts.existential_packs.contains_key(&expr) {
            return Some(vec![
                self.existential_pack_origin(state, expr)
                    .unwrap_or(LatentSource::Unavailable { expr }),
            ]);
        }
        self.latent_producer_sources(state, expr)
    }

    pub(super) fn latent_producer_sources(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> Option<Vec<LatentSource>> {
        if self.types.facts.bound_method_values.contains_key(&expr) {
            return Some(vec![
                self.bound_method_source(state, expr)
                    .unwrap_or(LatentSource::Unavailable { expr }),
            ]);
        }
        if let Some(sources) = self.definition_contract_sources(state, expr) {
            return Some(sources);
        }
        if self.inputs.std_callable_values.contains_key(&expr) {
            return Some(vec![LatentSource::StdReference {
                value: expr,
                environment: None,
            }]);
        }
        // These are previous-round origins for this lexical call site, not a
        // substitute for checking the invoked callable's complete behavior.
        let known_call_result = (self.context.view().index().expr_item.get(&expr).copied()
            == state.owner)
            .then(|| self.call_results.value_sources(expr))
            .flatten();
        if let Some(sources) = state.local_latent_values.value_sources(expr) {
            if sources.iter().all(|source| matches!(source, LatentSource::Unavailable { expr: missing } if *missing == expr))
                && let Some(known) = known_call_result {
                return Some(known.iter().copied().collect());
            }
            return Some(sources.iter().copied().collect());
        }
        if let Some(known) = known_call_result {
            return Some(known.iter().copied().collect());
        }
        match self.hir.exprs.get(expr)? {
            HirExpr::Lambda { .. } => {
                let owner = *self.context.view().index().expr_item.get(&expr)?;
                self.anonymous_flow_unit_for_expr(owner, expr)
                    .map(|unit| vec![LatentSource::Unit(unit)])
            }
            HirExpr::Path(path) => {
                let symbol = match &path.resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix?,
                    ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => return None,
                };
                if let Some(sources) = self.known_projection_sources(state, expr, &[]) {
                    return Some(sources.into_iter().collect());
                }
                if self.checked_empty_constructor_value(expr, symbol) {
                    return Some(Vec::new());
                }
                let mut sources = match self.latent_sources_for_symbol(state, symbol) {
                    Some(sources) => sources,
                    None if self.external_reference_metadata(expr).is_some() => {
                        vec![LatentSource::ExternalReference {
                            value: expr,
                            frozen: false,
                        }]
                    }
                    None => return None,
                };
                if let ResolveResult::PartiallyResolved(partial) = &path.resolution {
                    let fields = partial
                        .remaining
                        .iter()
                        .cloned()
                        .map(etas_code::descriptor::ValueProjectionStep::Field)
                        .collect::<Vec<_>>();
                    if !fields.is_empty()
                        && let Some(selected) =
                            self.project_parameter_origins(sources.iter().copied(), &fields)
                    {
                        sources = selected.into_iter().collect();
                    }
                }
                if self.types.facts.generic_instantiations.contains_key(&expr) {
                    for source in &mut sources {
                        if let LatentSource::Unit(unit) = *source {
                            *source = LatentSource::Reference {
                                unit,
                                value: expr,
                                environment: None,
                            };
                        }
                    }
                }
                Some(sources)
            }
            HirExpr::EmptySequence { .. } | HirExpr::EmptyRecordOrMap { .. } => {
                (self.expr_type_contains_flow(expr) || self.expr_type_has_parameters(expr))
                    .then(Vec::new)
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. }
                if elems.is_empty() =>
            {
                (self.expr_type_contains_flow(expr) || self.expr_type_has_parameters(expr))
                    .then(Vec::new)
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => self.union_latent_sources(
                elems
                    .iter()
                    .filter_map(|elem| self.latent_payload_sources(state, *elem)),
            ),
            HirExpr::Map { entries, .. } if entries.is_empty() => {
                (self.expr_type_contains_flow(expr) || self.expr_type_has_parameters(expr))
                    .then(Vec::new)
            }
            HirExpr::Map { entries, .. } => self.union_latent_sources(
                entries
                    .iter()
                    .filter_map(|entry| self.latent_payload_sources(state, entry.value)),
            ),
            HirExpr::Binary {
                lhs,
                op: etas_hir::HirBinaryOp::Add,
                rhs,
                ..
            } if self
                .types
                .facts
                .expr_types
                .get(&expr)
                .is_some_and(|ty| matches!(self.types.store.get(*ty), Some(Type::Array(_)))) =>
            {
                self.union_latent_sources(
                    [*lhs, *rhs]
                        .into_iter()
                        .filter_map(|value| self.latent_payload_sources(state, value)),
                )
            }
            HirExpr::ListCons { head, tail, .. } => self.union_latent_sources(
                [*head, *tail]
                    .into_iter()
                    .filter_map(|value| self.latent_payload_sources(state, value)),
            ),
            HirExpr::Record(record) => {
                self.union_latent_sources(record.fields.iter().filter_map(|field| match field {
                    etas_hir::HirFieldInit::Named { value, .. } => {
                        self.latent_payload_sources(state, *value)
                    }
                    etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                        let ResolveResult::Resolved(symbol) = resolution else {
                            return None;
                        };
                        self.latent_sources_for_symbol(state, *symbol).or_else(|| {
                            self.symbol_type_contains_flow(*symbol)
                                .then(|| vec![LatentSource::Unavailable { expr }])
                        })
                    }
                }))
            }
            HirExpr::Field { base, field, .. } => self
                .projected_sources(
                    *base,
                    &[etas_code::descriptor::ValueProjectionStep::Field(
                        field.clone(),
                    )],
                    state,
                )
                .map(|sources| sources.into_iter().collect())
                .or_else(|| self.latent_sources_for_expr(state, *base)),
            HirExpr::Index { base, .. } => {
                let step = match self
                    .types
                    .facts
                    .expr_types
                    .get(base)
                    .and_then(|ty| self.types.store.get(*ty))
                {
                    Some(Type::Map { .. }) => etas_code::descriptor::ValueProjectionStep::MapValue,
                    _ => etas_code::descriptor::ValueProjectionStep::SequenceElement,
                };
                self.projected_sources(*base, &[step], state)
                    .map(|sources| sources.into_iter().collect())
            }
            HirExpr::Try { expr: base, .. } => self.latent_sources_for_expr(state, *base),
            HirExpr::Call { callee, args, .. }
                if !self.latent_payload_arguments(*callee).is_empty() =>
            {
                self.union_latent_sources(
                    self.latent_payload_arguments(*callee)
                        .iter()
                        .filter_map(|position| args.get(*position).and_then(arg_expr))
                        .filter_map(|arg| self.latent_payload_sources(state, arg)),
                )
            }
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => self
                .union_latent_sources(
                    args.iter()
                        .filter_map(arg_expr)
                        .filter_map(|arg| self.latent_payload_sources(state, arg)),
                ),
            HirExpr::StageCompose { .. } => state
                .local_latent_values
                .value_sources(expr)
                .map(|sources| sources.iter().copied().collect()),
            _ => None,
        }
    }

    pub(crate) fn union_latent_sources(
        &self,
        sources: impl IntoIterator<Item = Vec<LatentSource>>,
    ) -> Option<Vec<LatentSource>> {
        let mut merged = std::collections::BTreeSet::new();
        let mut known = false;
        for sources in sources {
            known = true;
            merged.extend(sources);
        }
        known.then(|| merged.into_iter().collect())
    }

    pub(super) fn latent_payload_sources(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> Option<Vec<LatentSource>> {
        self.latent_sources_for_expr(state, expr).or_else(|| {
            (self.expr_type_contains_flow(expr) || self.expr_type_has_parameters(expr))
                .then(|| vec![LatentSource::Unavailable { expr }])
        })
    }

    pub(crate) fn latent_payload_arguments(&self, callee: HirExprId) -> &'static [usize] {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return &[];
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return &[];
        };
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return &[];
        };
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return &[];
        };
        self.std_registry
            .lookup_qualified(path)
            .and_then(|symbol| symbol.intrinsic.as_ref())
            .map(|descriptor| descriptor.payload_arguments())
            .unwrap_or(&[])
    }

    pub(crate) fn is_value_constructor_call(&self, callee: HirExprId) -> bool {
        if self
            .std_qualified_path_for_expr(callee)
            .and_then(|path| self.std_registry.lookup_qualified(&path))
            .is_some_and(|symbol| symbol.enum_owner.is_some() && symbol.intrinsic.is_none())
        {
            return true;
        }
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol) = self.type_symbols.canonical_symbol(self.hir, symbol) else {
            return false;
        };
        self.checked_enum_constructor(symbol)
            || self.hir.symbols.get(symbol).is_some_and(|symbol| {
                matches!(
                    symbol.kind,
                    etas_hir::SymbolKind::Type
                        | etas_hir::SymbolKind::Enum
                        | etas_hir::SymbolKind::EnumVariant
                )
            })
    }

    pub(crate) fn record_latent_sources_for_pat(
        &self,
        state: &mut EffectState,
        pat: HirPatId,
        sources: Vec<LatentSource>,
        value: Option<HirExprId>,
    ) -> Result<(), &'static str> {
        use etas_code::descriptor::ValueProjectionStep as Step;
        for (symbol, path) in self.pattern_projection_bindings(pat)? {
            let selected = if path
                .iter()
                .any(|step| matches!(step, Step::TupleElement(_)))
            {
                value
                    .and_then(|value| self.projected_sources(value, &path, state))
                    .or_else(|| self.project_parameter_origins(sources.iter().copied(), &path))
                    .map(|sources| sources.into_iter().collect::<Vec<_>>())
                    .unwrap_or_else(|| sources.clone())
            } else {
                sources.clone()
            };
            if self.symbol_type_contains_flow(symbol)
                || self
                    .symbol_value_type(symbol)
                    .is_some_and(|ty| self.type_has_parameters(ty))
                || self
                    .symbol_value_type(symbol)
                    .is_some_and(|ty| self.symbolic_contract_value(ty, &selected))
            {
                state
                    .local_latent_values
                    .record_symbol_sources(symbol, selected);
                if let Some(value) = value {
                    self.record_symbol_projections(state, symbol, value, &path);
                }
            }
        }
        Ok(())
    }

    pub(super) fn pattern_projection_bindings(
        &self,
        pat: HirPatId,
    ) -> Result<
        Vec<(
            etas_hir::SymbolId,
            Vec<etas_code::descriptor::ValueProjectionStep>,
        )>,
        &'static str,
    > {
        use etas_code::descriptor::ValueProjectionStep as Step;
        let mut bindings = Vec::new();
        let mut pending = vec![(pat, Vec::<Step>::new())];
        while let Some((pat, path)) = pending.pop() {
            let pat_data = self
                .hir
                .pats
                .get(pat)
                .ok_or("callable pattern has no checked HIR node")?;
            match pat_data {
                HirPat::Binding { symbol, .. } => bindings.push((*symbol, path)),
                HirPat::Tuple { elems, .. } => {
                    for (position, pat) in elems.iter().enumerate() {
                        let mut field = path.clone();
                        field.push(Step::TupleElement(position as u32));
                        pending.push((*pat, field));
                    }
                }
                HirPat::ListCons { head, tail, .. } => {
                    let mut head_path = path.clone();
                    head_path.push(Step::ListHead);
                    let mut tail_path = path;
                    tail_path.push(Step::ListTail);
                    pending.extend([(*head, head_path), (*tail, tail_path)]);
                }
                HirPat::Variant {
                    path: variant,
                    args,
                    ..
                } => {
                    let name = variant
                        .segments
                        .last()
                        .ok_or("variant pattern has no checked name")?
                        .name
                        .clone();
                    for (position, pat) in args.iter().enumerate() {
                        let mut field = path.clone();
                        field.push(Step::VariantPayload {
                            variant: name.clone(),
                            position: position as u32,
                        });
                        pending.push((*pat, field));
                    }
                }
                HirPat::Record { fields, .. } => {
                    for field in fields {
                        if let Some(pat) = field.pat {
                            let mut field_path = path.clone();
                            field_path.push(Step::Field(field.name.clone()));
                            pending.push((pat, field_path));
                        }
                    }
                }
                HirPat::EmptyList { .. }
                | HirPat::Wildcard { .. }
                | HirPat::Literal(_)
                | HirPat::Error { .. } => {}
            }
        }
        Ok(bindings)
    }

    pub(crate) fn top_level_anonymous_flow_unit(&self, item: HirItemId) -> Option<EffectUnit> {
        let etas_hir::HirItem::TopLevelLet(value) = self.hir.items.get(item)? else {
            return None;
        };
        let HirExpr::Lambda { body, .. } = self.hir.exprs.get(value.value?)? else {
            return None;
        };
        let body = match body {
            etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
            etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
        };
        Some(EffectUnit::AnonymousFlow {
            owner: item,
            value: value.value?,
            body,
        })
    }

    pub(crate) fn local_anonymous_flow_unit_for_symbol(
        &self,
        owner: HirItemId,
        symbol: etas_hir::SymbolId,
    ) -> Option<EffectUnit> {
        let body = self.owner_primary_block(owner)?;
        self.local_anonymous_flow_in_block(owner, body, symbol)
    }

    pub(crate) fn local_anonymous_flow_in_block(
        &self,
        owner: HirItemId,
        block: etas_hir::HirBlockId,
        symbol: etas_hir::SymbolId,
    ) -> Option<EffectUnit> {
        let block = self.hir.blocks.get(block)?;
        for stmt in &block.stmts {
            let Some(stmt) = self.hir.stmts.get(*stmt) else {
                continue;
            };
            match stmt {
                HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. }
                    if self.pat_binds_symbol(*pat, symbol) =>
                {
                    if let Some(unit) = self.anonymous_flow_unit_for_expr(owner, *value) {
                        return Some(unit);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub(crate) fn pat_binds_symbol(&self, pat: HirPatId, symbol: etas_hir::SymbolId) -> bool {
        let Some(pat) = self.hir.pats.get(pat) else {
            return false;
        };
        match pat {
            HirPat::Binding { symbol: bound, .. } => *bound == symbol,
            HirPat::ListCons { head, tail, .. } => {
                self.pat_binds_symbol(*head, symbol) || self.pat_binds_symbol(*tail, symbol)
            }
            HirPat::Tuple { elems, .. } | HirPat::Variant { args: elems, .. } => {
                elems.iter().any(|pat| self.pat_binds_symbol(*pat, symbol))
            }
            HirPat::Record { fields, .. } => fields
                .iter()
                .filter_map(|field| field.pat)
                .any(|pat| self.pat_binds_symbol(pat, symbol)),
            HirPat::EmptyList { .. }
            | HirPat::Wildcard { .. }
            | HirPat::Literal(_)
            | HirPat::Error { .. } => false,
        }
    }

    pub(crate) fn anonymous_flow_unit_for_expr(
        &self,
        owner: HirItemId,
        value: HirExprId,
    ) -> Option<EffectUnit> {
        match self.hir.exprs.get(value)? {
            HirExpr::Lambda { body, .. } => {
                let body = match body {
                    etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
                    etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
                };
                Some(EffectUnit::AnonymousFlow { owner, value, body })
            }
            HirExpr::Path(path) => {
                let symbol = match &path.resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix?,
                    ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => return None,
                };
                let symbol = self.hir.symbols.get(symbol)?;
                match symbol.def {
                    SymbolDef::Local {
                        initializer: Some(initializer),
                        ..
                    } => self.anonymous_flow_unit_for_expr(owner, initializer),
                    SymbolDef::TopLevelLet { item, .. } | SymbolDef::Item { item } => {
                        self.top_level_anonymous_flow_unit(item)
                    }
                    _ => None,
                }
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => elems
                .iter()
                .find_map(|expr| self.anonymous_flow_unit_for_expr(owner, *expr)),
            HirExpr::Map { entries, .. } => entries
                .iter()
                .find_map(|entry| self.anonymous_flow_unit_for_expr(owner, entry.value)),
            HirExpr::Record(record) => record.fields.iter().find_map(|field| match field {
                etas_hir::HirFieldInit::Named { value, .. } => {
                    self.anonymous_flow_unit_for_expr(owner, *value)
                }
                etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                    let ResolveResult::Resolved(symbol) = resolution else {
                        return None;
                    };
                    self.hir
                        .symbols
                        .get(*symbol)
                        .and_then(|symbol| match symbol.def {
                            SymbolDef::Local {
                                initializer: Some(initializer),
                                ..
                            } => self.anonymous_flow_unit_for_expr(owner, initializer),
                            _ => None,
                        })
                }
            }),
            HirExpr::Index { base, .. } | HirExpr::Field { base, .. } => {
                self.anonymous_flow_unit_for_expr(owner, *base)
            }
            HirExpr::Call { callee, args, .. }
                if !self.latent_payload_arguments(*callee).is_empty() =>
            {
                self.latent_payload_arguments(*callee)
                    .iter()
                    .filter_map(|position| args.get(*position).and_then(arg_expr))
                    .find_map(|arg| self.anonymous_flow_unit_for_expr(owner, arg))
            }
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => args
                .iter()
                .filter_map(arg_expr)
                .find_map(|arg| self.anonymous_flow_unit_for_expr(owner, arg)),
            HirExpr::StageCompose { .. } => Some(EffectUnit::AnonymousFlow {
                owner,
                value,
                body: EffectAnonymousFlowBody::Expr(value),
            }),
            _ => None,
        }
    }

    pub(crate) fn reject_effect_state(
        &mut self,
        span: Span,
        message: impl Into<String>,
        mut state: EffectState,
    ) -> EffectState {
        self.mark_rejected_at(span, message, &mut state);
        state
    }

    pub(super) fn mark_rejected_at(
        &mut self,
        span: Span,
        message: impl Into<String>,
        state: &mut EffectState,
    ) {
        self.mark_incomplete_at(span, message, state);
        state.summary.support =
            InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
    }

    pub(crate) fn flow_container_missing_latent_sources(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> bool {
        let Some(expr_data) = self.hir.exprs.get(expr) else {
            return false;
        };
        match expr_data {
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => elems
                .iter()
                .any(|elem| self.flow_expr_missing_latent_source(state, *elem)),
            HirExpr::Map { entries, .. } => entries
                .iter()
                .any(|entry| self.flow_expr_missing_latent_source(state, entry.value)),
            HirExpr::Record(record) => record.fields.iter().any(|field| match field {
                etas_hir::HirFieldInit::Named { value, .. } => {
                    self.flow_expr_missing_latent_source(state, *value)
                }
                etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                    let ResolveResult::Resolved(symbol) = resolution else {
                        return false;
                    };
                    self.symbol_type_contains_flow(*symbol)
                        && self
                            .latent_sources_for_symbol(state, *symbol)
                            .is_none_or(|sources| {
                                sources.iter().any(|source| {
                                    matches!(source, LatentSource::Unavailable { .. })
                                })
                            })
                }
            }),
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => args
                .iter()
                .filter_map(arg_expr)
                .any(|arg| self.flow_expr_missing_latent_source(state, arg)),
            _ => false,
        }
    }

    pub(crate) fn flow_expr_missing_latent_source(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> bool {
        if !self.expr_type_contains_flow(expr) {
            return false;
        }
        self.latent_sources_for_expr(state, expr)
            .is_none_or(|sources| {
                sources
                    .iter()
                    .any(|source| matches!(source, LatentSource::Unavailable { .. }))
            })
    }

    pub(crate) fn expr_type_contains_flow(&self, expr: HirExprId) -> bool {
        if self.types.facts.definition_operations.contains_key(&expr) {
            return true;
        }
        self.types
            .facts
            .expr_types
            .get(&expr)
            .is_some_and(|ty| self.type_contains_flow(*ty))
    }

    pub(crate) fn symbol_type_contains_flow(&self, symbol: etas_hir::SymbolId) -> bool {
        self.symbol_value_type(symbol)
            .is_some_and(|ty| self.type_contains_flow(ty))
    }

    pub(crate) fn type_contains_flow(&self, ty: etas_types::TypeId) -> bool {
        self.latent_type_shapes
            .get_or_init(|| super::latent_shape::LatentTypeShapes::build(self.types))
            .contains(ty)
    }

    pub(super) fn expr_type_has_parameters(&self, expr: HirExprId) -> bool {
        self.types
            .facts
            .expr_types
            .get(&expr)
            .is_some_and(|ty| self.type_has_parameters(*ty))
    }

    pub(super) fn type_has_parameters(&self, ty: TypeId) -> bool {
        self.latent_type_shapes
            .get_or_init(|| super::latent_shape::LatentTypeShapes::build(self.types))
            .has_parameters(ty)
    }

    pub(crate) fn is_deferred_first_class_param(&self, expr: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol_data) = self.hir.symbols.get(symbol) else {
            return false;
        };
        if !matches!(symbol_data.kind, etas_hir::SymbolKind::Param)
            && !matches!(symbol_data.def, SymbolDef::Param { .. })
        {
            return false;
        }
        self.flow_type_for_expr(expr)
            .is_some_and(|flow| flow.effects.is_none())
    }

    pub(crate) fn is_projected_flow_value(&self, expr: HirExprId) -> bool {
        matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::Index { .. } | HirExpr::Field { .. } | HirExpr::Call { .. })
        )
    }

    pub(crate) fn symbol_value_type(
        &self,
        symbol: etas_hir::SymbolId,
    ) -> Option<etas_types::TypeId> {
        match self
            .type_symbols
            .symbol_fact(self.hir, &self.types.facts, symbol)?
        {
            SymbolTypeFact::Param { ty }
            | SymbolTypeFact::Local { ty, .. }
            | SymbolTypeFact::Field { ty }
            | SymbolTypeFact::Value { ty }
            | SymbolTypeFact::TopLevelLet { ty, .. } => Some(*ty),
            _ => None,
        }
    }

    pub(crate) fn pure_type_constructor_call(&self, callee: HirExprId) -> bool {
        let Some(symbol) = self.callee_symbol(callee) else {
            return false;
        };
        if matches!(
            self.type_symbols
                .symbol_fact(self.hir, &self.types.facts, symbol),
            Some(
                SymbolTypeFact::Type { .. }
                    | SymbolTypeFact::NominalType { .. }
                    | SymbolTypeFact::TypeAlias { .. }
            )
        ) {
            return true;
        }
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return false;
        };
        matches!(
            symbol.kind,
            etas_hir::SymbolKind::Type
                | etas_hir::SymbolKind::TypeAlias
                | etas_hir::SymbolKind::Enum
                | etas_hir::SymbolKind::EnumVariant
        )
    }

    pub(crate) fn owner_primary_block(&self, item: HirItemId) -> Option<etas_hir::HirBlockId> {
        let body = self.context.item_primary_body(item)?;
        self.context.body_block(body)
    }

    pub(crate) fn record_latent_realization(&mut self, source: EffectUnit, call: HirExprId) {
        if let EffectUnit::AnonymousFlow { value, .. } = source {
            let realized_at = self.inputs.latent_realizations.entry(value).or_default();
            if !realized_at.contains(&call) {
                realized_at.push(call);
            }
        }
    }
}
