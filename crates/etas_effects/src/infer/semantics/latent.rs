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
            self.hir.exprs.get(value.value),
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
                if self.checked_empty_constructor_value(expr, symbol) {
                    return Some(Vec::new());
                }
                let mut sources = self.latent_sources_for_symbol(state, symbol)?;
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
                self.expr_type_contains_flow(expr).then(Vec::new)
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. }
                if elems.is_empty() =>
            {
                self.expr_type_contains_flow(expr).then(Vec::new)
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
                self.expr_type_contains_flow(expr).then(Vec::new)
            }
            HirExpr::Map { entries, .. } => self.union_latent_sources(
                entries
                    .iter()
                    .filter_map(|entry| self.latent_payload_sources(state, entry.value)),
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
            HirExpr::Index { base, .. } | HirExpr::Field { base, .. } => {
                self.latent_sources_for_expr(state, *base)
            }
            HirExpr::Call { callee, args, .. } if self.is_latent_transparent_call(*callee) => args
                .first()
                .and_then(arg_expr)
                .and_then(|arg| self.latent_sources_for_expr(state, arg)),
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

    fn latent_payload_sources(
        &self,
        state: &EffectState,
        expr: HirExprId,
    ) -> Option<Vec<LatentSource>> {
        self.latent_sources_for_expr(state, expr).or_else(|| {
            self.expr_type_contains_flow(expr)
                .then(|| vec![LatentSource::Unavailable { expr }])
        })
    }

    pub(crate) fn is_latent_transparent_call(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        let Some(symbol) = self.hir.symbols.get(symbol) else {
            return false;
        };
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return false;
        };
        self.std_registry
            .lookup_qualified(path)
            .and_then(|symbol| symbol.intrinsic.as_ref())
            .is_some_and(|descriptor| {
                descriptor.latent_effect == etas_std::IntrinsicLatentEffect::TransparentFirstArg
            })
    }

    pub(crate) fn is_value_constructor_call(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return false;
        };
        self.hir.symbols.get(symbol).is_some_and(|symbol| {
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
    ) -> Result<(), &'static str> {
        let mut pending = vec![pat];
        while let Some(pat) = pending.pop() {
            let pat_data = self
                .hir
                .pats
                .get(pat)
                .ok_or("callable pattern has no checked HIR node")?;
            match pat_data {
                HirPat::Binding { symbol, .. } => {
                    state
                        .local_latent_values
                        .record_symbol_sources(*symbol, sources.clone());
                }
                HirPat::Tuple { elems, .. } | HirPat::Variant { args: elems, .. } => {
                    pending.extend(elems)
                }
                HirPat::Record { fields, .. } => {
                    for field in fields {
                        if let Some(pat) = field.pat {
                            pending.push(pat);
                        }
                    }
                }
                HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => {}
            }
        }
        Ok(())
    }

    pub(crate) fn top_level_anonymous_flow_unit(&self, item: HirItemId) -> Option<EffectUnit> {
        let etas_hir::HirItem::TopLevelLet(value) = self.hir.items.get(item)? else {
            return None;
        };
        let HirExpr::Lambda { body, .. } = self.hir.exprs.get(value.value)? else {
            return None;
        };
        let body = match body {
            etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
            etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
        };
        Some(EffectUnit::AnonymousFlow {
            owner: item,
            value: value.value,
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
            HirPat::Tuple { elems, .. } | HirPat::Variant { args: elems, .. } => {
                elems.iter().any(|pat| self.pat_binds_symbol(*pat, symbol))
            }
            HirPat::Record { fields, .. } => fields
                .iter()
                .filter_map(|field| field.pat)
                .any(|pat| self.pat_binds_symbol(pat, symbol)),
            HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => false,
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
            HirExpr::Call { callee, args, .. } if self.is_latent_transparent_call(*callee) => args
                .first()
                .and_then(arg_expr)
                .and_then(|arg| self.anonymous_flow_unit_for_expr(owner, arg)),
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
        self.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
        state.mark_incomplete();
        state.summary.support =
            InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
        state
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
        if let Some(found) = self.flow_type_presence.borrow().get(&ty) {
            return *found;
        }
        let root = ty;
        let mut pending = vec![ty];
        let mut visited = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            if let Some(found) = self.flow_type_presence.borrow().get(&ty).copied() {
                if found {
                    return true;
                }
                continue;
            }
            match self.types.store.get(ty) {
                Some(Type::Function(_)) => {
                    self.flow_type_presence.borrow_mut().insert(root, true);
                    return true;
                }
                Some(
                    Type::Array(inner)
                    | Type::List(inner)
                    | Type::Set(inner)
                    | Type::Slice(inner)
                    | Type::Option(inner)
                    | Type::Schema(inner)
                    | Type::Message(inner)
                    | Type::MemorySelection(inner)
                    | Type::MemoryRegion(inner)
                    | Type::Refined { base: inner, .. }
                    | Type::Trust { inner, .. },
                ) => pending.push(*inner),
                Some(
                    Type::Map { key, value }
                    | Type::Store { key, value }
                    | Type::Result {
                        ok: key,
                        err: value,
                    },
                ) => pending.extend([*key, *value]),
                Some(Type::Record(record)) => {
                    pending.extend(record.fields.iter().map(|field| field.ty))
                }
                Some(Type::Tuple(types)) => pending.extend(types),
                Some(Type::Nominal(nominal)) => pending.extend(nominal.representation),
                Some(Type::Applied { constructor, args }) => {
                    // Traverse the finite declaration graph and concrete arguments,
                    // never repeatedly instantiate recursive nominal applications.
                    pending.push(etas_types::TypeId(constructor.0));
                    pending.extend(args);
                }
                Some(Type::Enum(_)) => match self.types.facts.enum_layouts.get(&ty) {
                    Some(layout) => pending.extend(
                        layout
                            .variants
                            .iter()
                            .flat_map(|variant| variant.fields.iter())
                            .copied(),
                    ),
                    // Incomplete layout facts cannot prove the absence of callables.
                    None => return true,
                },
                Some(
                    Type::Primitive(_)
                    | Type::IntegerLiteral { .. }
                    | Type::Var(_)
                    | Type::Range { .. }
                    | Type::Handler(_)
                    | Type::Named(_)
                    | Type::Prompt
                    | Type::PromptPart
                    | Type::MemoryPlace(_)
                    | Type::ResourceHandle(_),
                ) => {}
                None => return true,
            }
        }
        let mut cache = self.flow_type_presence.borrow_mut();
        for ty in visited {
            cache.insert(ty, false);
        }
        false
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
        if let SymbolDef::ImportAlias {
            path,
            origin: etas_hir::ImportAliasOrigin::SourceImport,
        } = &symbol.def
        {
            return self.source_type_item_for_path(path).is_some();
        }
        matches!(
            symbol.kind,
            etas_hir::SymbolKind::Type
                | etas_hir::SymbolKind::TypeAlias
                | etas_hir::SymbolKind::Enum
                | etas_hir::SymbolKind::EnumVariant
        )
    }

    pub(crate) fn source_type_item_for_path(&self, path: &[String]) -> Option<HirItemId> {
        let (name, module_path) = path.split_last()?;
        for (item, hir_item) in self.hir.items.iter() {
            let symbol = match hir_item {
                etas_hir::HirItem::TypeAlias(item) => item.symbol,
                etas_hir::HirItem::Type(item) => item.symbol,
                etas_hir::HirItem::Enum(item) => item.symbol,
                _ => continue,
            };
            let Some(symbol) = self.hir.symbols.get(symbol) else {
                continue;
            };
            if symbol.name != *name {
                continue;
            }
            let Some(module) = self.hir.modules_arena.get(symbol.defining_module) else {
                continue;
            };
            let Some(module_name) = &module.name else {
                continue;
            };
            let actual = module_name
                .segments
                .iter()
                .map(|segment| segment.name.as_str())
                .collect::<Vec<_>>();
            if actual == module_path.iter().map(String::as_str).collect::<Vec<_>>() {
                return Some(item);
            }
        }
        None
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
