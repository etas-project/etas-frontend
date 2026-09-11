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

            if let Some(summary) = self.summary_for_callable_expr(stage.expr, span) {
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
        &self,
        expr: HirExprId,
        span: Span,
    ) -> Option<EffectSummary> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return self.summary_for_flow_expr_type(expr, span);
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return self.summary_for_flow_expr_type(expr, span);
        };
        let symbol = self.hir.symbols.get(symbol)?;
        match &symbol.def {
            etas_hir::SymbolDef::Item { item } => {
                if matches!(self.hir.items.get(*item), Some(etas_hir::HirItem::Agent(_))) {
                    return self.summary_for_agent_call(*item, span);
                }
                self.inputs
                    .unit_effects
                    .get(&EffectUnit::Item(*item))
                    .cloned()
                    .or_else(|| self.summary_for_item_public_effects(*item, span))
            }
            etas_hir::SymbolDef::ImportAlias {
                path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => self
                .source_item_for_path(path)
                .and_then(|item| {
                    if matches!(self.hir.items.get(item), Some(etas_hir::HirItem::Agent(_))) {
                        self.summary_for_agent_call(item, span)
                    } else {
                        self.summary_for_item_public_effects(item, span)
                    }
                })
                .or_else(|| self.external_summary_for_path(path, span)),
            _ => self.summary_for_flow_expr_type(expr, span),
        }
    }

    pub(crate) fn source_item_for_path(&self, path: &[String]) -> Option<HirItemId> {
        let (name, module_path) = path.split_last()?;
        for (item, hir_item) in self.hir.items.iter() {
            let Some(symbol) = (match hir_item {
                etas_hir::HirItem::Flow(flow) => Some(flow.symbol),
                etas_hir::HirItem::Agent(agent) => Some(agent.symbol),
                etas_hir::HirItem::Tool(tool) => Some(tool.symbol),
                etas_hir::HirItem::TopLevelLet(value) => Some(value.symbol),
                _ => None,
            }) else {
                continue;
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

    pub(crate) fn agent_infer_summary(&self, item: HirItemId, span: Span) -> Option<EffectSummary> {
        let Some(etas_hir::HirItem::Agent(agent)) = self.hir.items.get(item) else {
            return None;
        };
        let Some(etas_types::SymbolTypeFact::Agent { signature }) =
            self.types.facts.symbol_types.get(&agent.symbol)
        else {
            return None;
        };
        let action = self
            .registry
            .core_action(CoreEffect::Agentic, AGENTIC_INFER_ACTION)?;
        let path = self.item_effect_path(item)?;
        let mut summary = EffectSummary::local();
        let effect = Effect::AppliedAction(ActionInstanceRef {
            action: action.clone(),
            args: vec![
                EffectArgRef::Path(path),
                EffectArgRef::Type(signature.output),
            ],
        });
        summary.record_requested_action(effect.clone());
        summary.record_default_handled_action(effect.clone());
        summary.record_action_trace_event(effect, span, crate::ActionEventSource::AgentCall);
        if let Some(reason) = self
            .registry
            .runtime_requirement_reason_for_action_ref(&action)
        {
            summary.require_runtime(reason);
        }
        Some(summary)
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
