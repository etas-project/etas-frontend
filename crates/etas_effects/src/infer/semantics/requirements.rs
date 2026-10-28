use super::engine::EffectSemantics;
use super::shared::*;

impl EffectSemantics<'_> {
    pub(crate) fn limit_requirement_from_expr(&self, expr: HirExprId) -> Option<LimitRequirement> {
        let (callee, args) = match self.hir.exprs.get(expr)? {
            HirExpr::Call { callee, args, .. } => (*callee, args.as_slice()),
            _ => return None,
        };
        let HirExpr::Path(path) = self.hir.exprs.get(callee)? else {
            return None;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.hir.symbols.get(symbol)?;
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return None;
        };
        let std_symbol = self.std_registry.lookup_qualified(path)?;
        let StdDecl::Requirement(requirement) = &std_symbol.decl else {
            return None;
        };
        let RequirementSemantics::Limit(std_kind) = requirement.semantics else {
            return None;
        };
        let kind = limit_kind_from_std(std_kind);
        let value = self.limit_value_from_args(kind, args)?;
        Some(LimitRequirement {
            kind,
            budget: kind.budget_kind(),
            value: Some(value),
        })
    }

    pub(crate) fn limit_value_from_args(
        &self,
        kind: LimitKind,
        args: &[HirArg],
    ) -> Option<LimitValue> {
        if args.len() != 1 {
            return None;
        }
        let expr = match args.first()? {
            HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
        };
        let HirExpr::Literal(HirLiteral::Int { text, .. }) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let value = text.replace('_', "").parse::<u64>().ok()?;
        Some(match kind {
            LimitKind::Iterations
            | LimitKind::Tokens
            | LimitKind::ContextTokens
            | LimitKind::Attempts => LimitValue::Count(value),
            LimitKind::Cost => LimitValue::MoneyMicros {
                amount: u128::from(value),
                currency: "USD".to_owned(),
            },
            LimitKind::WallTime => LimitValue::DurationMillis(value),
        })
    }

    pub(crate) fn apply_stage_effects(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: EffectState,
    ) -> EffectState {
        let stages = match self.hir.exprs.get(expr) {
            Some(HirExpr::StageCompose { stages, .. }) | Some(HirExpr::Pipeline { stages, .. }) => {
                stages.clone()
            }
            _ => Vec::new(),
        };
        if let Some(unit) = state.unit {
            let mut captured = super::state::LatentEnv::default();
            for stage in &stages {
                if let Some(sources) = self.latent_sources_for_expr(&state, stage.expr) {
                    captured.record_value_sources(stage.expr, sources);
                }
            }
            self.discovered_value_captures
                .entry(unit)
                .or_default()
                .join_assign(&captured);
        }
        for stage in stages {
            let mut parsed_limits = RequirementSet::new();
            for limit in stage.limits {
                let Some(requirement) = self.limit_requirement_from_expr(limit) else {
                    state = self.incomplete_at(
                        span,
                        "pipeline stage limit requires a checked standard limit constructor",
                        state,
                    );
                    continue;
                };
                parsed_limits.insert(RequirementFact::Limit(requirement));
            }
            state.summary.requirements.union_assign(&parsed_limits);

            if let Some(summary) = self.summary_for_callable_expr(&state, stage.expr, span) {
                state.summary.seq_assign(&summary);
            } else if self.is_typed_flow_expr(stage.expr) {
                state = self.incomplete_at(
                    span,
                    "pipeline stage requires materialized callable effect facts",
                    state,
                );
            }
        }
        state
    }

    pub(crate) fn summary_for_callable_expr(
        &mut self,
        state: &EffectState,
        expr: HirExprId,
        span: Span,
    ) -> Option<EffectSummary> {
        if let Some(sources) = self.latent_sources_for_expr(state, expr) {
            let mut combined: Option<EffectSummary> = None;
            for source in sources {
                let summary = self.latent_source_summary(source, span).ok()?;
                if let Some(existing) = &mut combined {
                    existing.join_branch(&summary);
                } else {
                    combined = Some(summary);
                }
            }
            return combined;
        }
        if let Some(summary) = self.std_flow_summary(expr, expr, span) {
            return Some(summary);
        }
        let symbol = self.callee_symbol(expr)?;
        let SymbolDef::ImportAlias { path, .. } = &self.hir.symbols.get(symbol)?.def else {
            return None;
        };
        self.external_summary_for_path(path, span)
    }

    pub(crate) fn source_item_for_symbol(&self, symbol: etas_hir::SymbolId) -> Option<HirItemId> {
        let symbol = self.hir.canonical_source_symbol(symbol)?;
        let item = match self.hir.symbols.get(symbol)?.def {
            SymbolDef::Item { item } | SymbolDef::TopLevelLet { item, .. } => item,
            _ => return None,
        };
        (self.hir.items.get(item)?.symbol() == Some(symbol)).then_some(item)
    }

    pub(crate) fn item_effect_path(&self, item: HirItemId) -> Option<Vec<String>> {
        let symbol = match self.hir.items.get(item)? {
            etas_hir::HirItem::Flow(flow) => flow.symbol,
            etas_hir::HirItem::Agent(agent) => agent.symbol,
            etas_hir::HirItem::Tool(tool) => tool.symbol,
            etas_hir::HirItem::TopLevelLet(value) => value.symbol,
            _ => return None,
        };
        let symbol = self.hir.symbols.get(symbol)?;
        let mut path = self
            .hir
            .modules_arena
            .get(symbol.defining_module)?
            .name
            .as_ref()
            .map(|module| {
                module
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        path.push(symbol.name.clone());
        Some(path)
    }

    pub(crate) fn incomplete_summary(
        &mut self,
        span: Span,
        message: &'static str,
    ) -> EffectSummary {
        self.diagnostics.push(Diagnostic::effect_check(
            EffectDiagnosticCode::IncompleteEffectFacts,
            span,
            message,
        ));
        let mut summary = EffectSummary::local();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::UnresolvedEffect);
        summary
    }
}
