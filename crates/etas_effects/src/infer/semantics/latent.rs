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
    ) -> Option<Vec<EffectUnit>> {
        if let Some(sources) = state.local_latent_values.value_sources(expr) {
            if !sources.is_empty() {
                return Some(sources.iter().copied().collect());
            }
        }
        match self.hir.exprs.get(expr)? {
            HirExpr::Path(path) => {
                let symbol = match &path.resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix?,
                    ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => return None,
                };
                if let Some(sources) = state.local_latent_values.symbol_sources(symbol) {
                    if !sources.is_empty() {
                        return Some(sources.iter().copied().collect());
                    }
                }
                let symbol_data = self.hir.symbols.get(symbol)?;
                match symbol_data.def {
                    SymbolDef::Item { item } => match self.hir.items.get(item) {
                        Some(
                            etas_hir::HirItem::Flow(_)
                            | etas_hir::HirItem::Agent(_)
                            | etas_hir::HirItem::Tool(_),
                        ) => Some(vec![EffectUnit::Item(item)]),
                        _ => self
                            .top_level_anonymous_flow_unit(item)
                            .map(|unit| vec![unit]),
                    },
                    SymbolDef::TopLevelLet { item, .. } => self
                        .top_level_anonymous_flow_unit(item)
                        .map(|unit| vec![unit]),
                    _ => None,
                }
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Tuple { elems, .. }
            | HirExpr::Set { elems, .. } => self.union_latent_sources(
                elems
                    .iter()
                    .filter_map(|elem| self.latent_sources_for_expr(state, *elem)),
            ),
            HirExpr::Map { entries, .. } => self.union_latent_sources(
                entries
                    .iter()
                    .filter_map(|entry| self.latent_sources_for_expr(state, entry.value)),
            ),
            HirExpr::Record(record) => {
                self.union_latent_sources(record.fields.iter().filter_map(|field| match field {
                    etas_hir::HirFieldInit::Named { value, .. } => {
                        self.latent_sources_for_expr(state, *value)
                    }
                    etas_hir::HirFieldInit::Shorthand { resolution, .. } => {
                        let ResolveResult::Resolved(symbol) = resolution else {
                            return None;
                        };
                        state
                            .local_latent_values
                            .symbol_sources(*symbol)
                            .map(|sources| sources.iter().copied().collect())
                    }
                }))
            }
            HirExpr::Index { base, .. } | HirExpr::Field { base, .. } => {
                self.latent_sources_for_expr(state, *base)
            }
            HirExpr::Call { callee, args, .. } if self.is_wrapper_or_unwrap_call(*callee) => args
                .first()
                .and_then(arg_expr)
                .and_then(|arg| self.latent_sources_for_expr(state, arg)),
            HirExpr::Call { callee, args, .. } if self.is_value_constructor_call(*callee) => self
                .union_latent_sources(
                    args.iter()
                        .filter_map(arg_expr)
                        .filter_map(|arg| self.latent_sources_for_expr(state, arg)),
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
        sources: impl IntoIterator<Item = Vec<EffectUnit>>,
    ) -> Option<Vec<EffectUnit>> {
        let mut merged = std::collections::BTreeSet::new();
        for sources in sources {
            merged.extend(sources);
        }
        (!merged.is_empty()).then(|| merged.into_iter().collect())
    }

    pub(crate) fn is_wrapper_or_unwrap_call(&self, callee: HirExprId) -> bool {
        let Some(HirExpr::Path(path)) = self.hir.exprs.get(callee) else {
            return false;
        };
        path.segments
            .last()
            .is_some_and(|segment| matches!(segment.name.as_str(), "Some" | "Ok" | "unwrap"))
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
        sources: Vec<EffectUnit>,
    ) {
        let Some(pat_data) = self.hir.pats.get(pat) else {
            return;
        };
        match pat_data {
            HirPat::Binding { symbol, .. } => {
                state
                    .local_latent_values
                    .record_symbol_sources(*symbol, sources);
            }
            HirPat::Tuple { elems, .. } | HirPat::Variant { args: elems, .. } => {
                for elem in elems {
                    self.record_latent_sources_for_pat(state, *elem, sources.clone());
                }
            }
            HirPat::Record { fields, .. } => {
                for field in fields {
                    if let Some(pat) = field.pat {
                        self.record_latent_sources_for_pat(state, pat, sources.clone());
                    }
                }
            }
            HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => {}
        }
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
            HirExpr::Call { callee, args, .. } if self.is_wrapper_or_unwrap_call(*callee) => args
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
                        && state.local_latent_values.symbol_sources(*symbol).is_none()
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
        self.latent_sources_for_expr(state, expr).is_none()
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
        match self.types.store.get(ty) {
            Some(Type::Function(_)) => true,
            Some(Type::Array(inner))
            | Some(Type::List(inner))
            | Some(Type::Set(inner))
            | Some(Type::Slice(inner))
            | Some(Type::Option(inner))
            | Some(Type::Schema(inner))
            | Some(Type::Message(inner))
            | Some(Type::MemorySelection(inner))
            | Some(Type::MemoryRegion(inner))
            | Some(Type::Refined { base: inner, .. })
            | Some(Type::Trust { inner, .. }) => self.type_contains_flow(*inner),
            Some(Type::Map { value, .. }) => self.type_contains_flow(*value),
            Some(Type::Result { ok, err }) => {
                self.type_contains_flow(*ok) || self.type_contains_flow(*err)
            }
            Some(Type::Record(record)) => record
                .fields
                .iter()
                .any(|field| self.type_contains_flow(field.ty)),
            Some(Type::Tuple(types)) => types.iter().any(|ty| self.type_contains_flow(*ty)),
            Some(Type::Store { key, value }) => {
                self.type_contains_flow(*key) || self.type_contains_flow(*value)
            }
            Some(Type::Applied { args, .. }) => args.iter().any(|ty| self.type_contains_flow(*ty)),
            Some(
                Type::Primitive(_)
                | Type::IntegerLiteral { .. }
                | Type::Var(_)
                | Type::Range { .. }
                | Type::Enum(_)
                | Type::Handler(_)
                | Type::Named(_)
                | Type::Nominal(_)
                | Type::Prompt
                | Type::PromptPart
                | Type::MemoryPlace(_)
                | Type::ResourceHandle(_),
            )
            | None => false,
        }
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

    pub(crate) fn open_effect_parameter_call(&self, expr: HirExprId) -> Option<String> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.hir.symbols.get(symbol)?;
        if !matches!(symbol.kind, etas_hir::SymbolKind::Param)
            && !matches!(symbol.def, SymbolDef::Param { .. })
        {
            return None;
        }
        self.flow_type_for_expr(expr)?
            .effects
            .as_ref()?
            .tail
            .as_ref()?;
        Some(symbol.name.clone())
    }

    pub(crate) fn is_projected_flow_value(&self, expr: HirExprId) -> bool {
        matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::Index { .. } | HirExpr::Field { .. } | HirExpr::Call { .. })
        )
    }

    pub(crate) fn apply_deferred_first_class_call_sites(
        &mut self,
        context: UnitContext<EffectUnit>,
        site: CallSite<EffectUnit>,
        callee: EffectUnit,
        mut state: EffectState,
    ) -> EffectState {
        let EffectUnit::Item(item) = callee else {
            return state;
        };
        let deferred = self
            .inputs
            .deferred_first_class_calls
            .get(&callee)
            .cloned()
            .unwrap_or_default();
        let returned_flow = self.returned_anonymous_flow_unit(item);
        let returned_deferred = returned_flow.and_then(|unit| {
            self.inputs
                .deferred_first_class_calls
                .get(&unit)
                .cloned()
                .map(|calls| (unit, calls))
        });
        if let Some(returned_unit) = returned_flow
            && returned_deferred.is_none()
        {
            state
                .local_latent_values
                .record_value(site.call, returned_unit);
        }
        if deferred.is_empty() && returned_deferred.is_none() {
            if returned_flow.is_none()
                && self
                    .flow_type_for_expr(site.call)
                    .is_some_and(|flow| flow.effects.is_none())
            {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    site.span,
                    "returned first-class flow propagation requires a checked latent return source",
                ));
                state.mark_incomplete();
                state.summary.support =
                    InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
                return state;
            }
            return state;
        }
        let call_args =
            match self.hir.exprs.get(site.call) {
                Some(HirExpr::Call { args, .. }) => args.clone(),
                _ => return self.incomplete_at(
                    site.span,
                    "first-class flow call specialization requires a materialized call expression",
                    state,
                ),
            };
        let mut specialized = EffectSummary::local();
        let mut solved_any = false;
        for deferred_call in &deferred {
            match self.specialized_summary_for_deferred_call(
                item,
                *deferred_call,
                &call_args,
                &state,
                site.span,
            ) {
                DeferredSpecialization::Solved(summary) => {
                    let call_unit = EffectUnit::FirstClassFlowCall {
                        owner: item,
                        call: *deferred_call,
                    };
                    self.inputs
                        .unit_effects
                        .insert(call_unit, (*summary).clone());
                    self.inputs.solved_deferred_units.insert(call_unit);
                    specialized.seq_assign(&summary);
                    solved_any = true;
                }
                DeferredSpecialization::Rejected { message } => {
                    state = self.incomplete_at(site.span, message, state);
                    self.reject_deferred_unit(callee);
                }
            }
        }
        if solved_any {
            state.summary.seq_assign(&specialized);
            self.inputs.solved_deferred_units.insert(callee);
        }

        if let Some((returned_unit, returned_calls)) = returned_deferred {
            let mut returned_summary = EffectSummary::local();
            let mut returned_solved = false;
            for deferred_call in returned_calls {
                match self.specialized_summary_for_deferred_call(
                    item,
                    deferred_call,
                    &call_args,
                    &state,
                    site.span,
                ) {
                    DeferredSpecialization::Solved(summary) => {
                        let deferred_call_unit = EffectUnit::FirstClassFlowCall {
                            owner: item,
                            call: deferred_call,
                        };
                        self.inputs
                            .unit_effects
                            .insert(deferred_call_unit, (*summary).clone());
                        self.inputs.solved_deferred_units.insert(deferred_call_unit);
                        returned_summary.seq_assign(&summary);
                        returned_solved = true;
                    }
                    DeferredSpecialization::Rejected { .. } => {
                        state = self.incomplete_at(
                            site.span,
                            "returned first-class flow propagation requires a checked latent return source",
                            state,
                        );
                        self.reject_deferred_unit(returned_unit);
                    }
                }
            }
            if returned_solved {
                let owner = context.unit.owner().unwrap_or(item);
                let call_unit = EffectUnit::FirstClassFlowCall {
                    owner,
                    call: site.call,
                };
                self.inputs.unit_effects.insert(call_unit, returned_summary);
                self.inputs.solved_deferred_units.insert(returned_unit);
                state.local_latent_values.record_value(site.call, call_unit);
            }
        }

        state
    }

    pub(crate) fn specialized_summary_for_deferred_call(
        &mut self,
        callee_item: HirItemId,
        deferred_call: HirExprId,
        call_args: &[HirArg],
        state: &EffectState,
        span: Span,
    ) -> DeferredSpecialization {
        let Some(param_symbol) = self.deferred_call_param_symbol(deferred_call) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires a checked parameter target",
            };
        };
        let Some(param_ty) = self.symbol_value_type(param_symbol) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires checked parameter type facts",
            };
        };
        let Some(Type::Function(flow)) = self.types.store.get(param_ty) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires a checked function parameter type",
            };
        };
        if let Some(row) = &flow.effects
            && row.tail.is_none()
        {
            return DeferredSpecialization::Solved(Box::new(
                self.summary_for_invoked_flow_row(self.row_from_type_ref(row), span),
            ));
        }
        let Some(index) = self.param_index(callee_item, param_symbol) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires checked parameter type facts",
            };
        };
        let Some(arg) = call_args.get(index).and_then(arg_expr) else {
            return DeferredSpecialization::Rejected {
                message: "first-class flow call specialization requires a matching call-site argument",
            };
        };
        if let Some(sources) = self.latent_sources_for_expr(state, arg) {
            let mut summary = EffectSummary::local();
            for source in sources {
                let Some(source_summary) = self.inputs.unit_effects.get(&source).cloned() else {
                    return DeferredSpecialization::Rejected {
                        message: "first-class flow call requires a checked latent effect fact",
                    };
                };
                summary.seq_assign(&source_summary);
                self.record_latent_realization(source, deferred_call);
            }
            return DeferredSpecialization::Solved(Box::new(summary));
        }
        if let Some(arg_summary) = self.summary_for_flow_expr_type(arg, span) {
            return DeferredSpecialization::Solved(Box::new(arg_summary));
        }
        DeferredSpecialization::Rejected {
            message: "first-class flow call requires a checked latent effect fact or an explicit function effect row",
        }
    }

    pub(crate) fn deferred_call_param_symbol(&self, call: HirExprId) -> Option<etas_hir::SymbolId> {
        let HirExpr::Call { callee, .. } = self.hir.exprs.get(call)? else {
            return None;
        };
        let HirExpr::Path(path) = self.hir.exprs.get(*callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        Some(symbol)
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

    pub(crate) fn param_index(&self, item: HirItemId, symbol: etas_hir::SymbolId) -> Option<usize> {
        match self.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => flow.params.iter().position(|param| *param == symbol),
            etas_hir::HirItem::Agent(agent) => {
                agent.params.iter().position(|param| *param == symbol)
            }
            etas_hir::HirItem::Tool(tool) => tool.params.iter().position(|param| *param == symbol),
            _ => None,
        }
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

    pub(crate) fn returned_anonymous_flow_unit(&self, item: HirItemId) -> Option<EffectUnit> {
        let block = self.owner_primary_block(item)?;
        let block = self.hir.blocks.get(block)?;
        for stmt in &block.stmts {
            let Some(HirStmt::Return {
                value: Some(value), ..
            }) = self.hir.stmts.get(*stmt)
            else {
                continue;
            };
            if let Some(unit) = self.anonymous_flow_unit_for_expr(item, *value) {
                return Some(unit);
            }
        }
        block
            .final_expr
            .and_then(|expr| self.anonymous_flow_unit_for_expr(item, expr))
    }

    pub(crate) fn owner_primary_block(&self, item: HirItemId) -> Option<etas_hir::HirBlockId> {
        let body = self.context.item_primary_body(item)?;
        self.context.body_block(body)
    }

    pub(crate) fn reject_deferred_unit(&mut self, unit: EffectUnit) {
        let summary = self.inputs.unit_effects.entry(unit).or_default();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
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
