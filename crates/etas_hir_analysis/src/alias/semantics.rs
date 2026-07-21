use std::collections::{BTreeMap, BTreeSet};

use etas_core::{AnalysisDiagnosticCode, Diagnostic, Span};
use etas_hir::{
    HirArg, HirExpr, HirExprId, HirFieldInit, HirGenericArg, HirHandlerArmId, HirItem, HirPat,
    HirPatId, HirProgram, HirStmt, HirStmtId, ResolveResult, ResolvedActionRef, SymbolId,
};

use crate::{
    HirAnalysisContext,
    interprocedural::{
        CallSite, CallTarget, HirAnalysisBody, InterproceduralSemantics, UnitContext,
    },
    intraprocedural::{AnalysisStep, Control, HirAnalysisSemantics},
    unit::HirSemanticUnit,
};

use super::{
    AliasContext, AliasPrecisionConfig, ContextualAliasUnit,
    config::{AllocationSensitivity, FieldSensitivity, FlowSensitivity, HeapModel, UpdatePolicy},
    domain::{AliasDomain, AliasValue},
    facts::AliasFacts,
    oracle::{AliasCallTarget, AliasOracle, AllocationKind},
    place::{AliasTarget, AllocationSiteKind, Place, Projection},
    solver::{AliasConstraint, AliasConstraintFrame, AliasVar},
    summary::{AliasSummary, AliasValueExpr, AliasWriteEffect},
};

pub(crate) struct AliasSemantics<'a, O>
where
    O: AliasOracle,
{
    hir: &'a HirProgram,
    context: HirAnalysisContext,
    config: AliasPrecisionConfig,
    oracle: O,
    lambda_units: BTreeMap<HirExprId, HirSemanticUnit>,
    handler_units: BTreeMap<HirHandlerArmId, HirSemanticUnit>,
    captures: BTreeMap<ContextualAliasUnit, BTreeMap<SymbolId, AliasValueExpr>>,
    semantic_captures: BTreeMap<HirSemanticUnit, BTreeMap<SymbolId, AliasValueExpr>>,
    facts: AliasFacts,
    diagnostics: Vec<Diagnostic>,
}

impl<'a, O> AliasSemantics<'a, O>
where
    O: AliasOracle,
{
    pub(crate) fn new(
        hir: &'a HirProgram,
        units: &[HirSemanticUnit],
        config: AliasPrecisionConfig,
        oracle: O,
    ) -> Self {
        let mut lambda_units = BTreeMap::new();
        let mut handler_units = BTreeMap::new();
        for unit in units {
            match *unit {
                HirSemanticUnit::AnonymousFlow { expr, .. } => {
                    lambda_units.insert(expr, *unit);
                }
                HirSemanticUnit::HandlerArm(arm) => {
                    handler_units.insert(arm, *unit);
                }
                HirSemanticUnit::Item(_) | HirSemanticUnit::TopLevelLet(_) => {}
            }
        }
        Self {
            hir,
            context: HirAnalysisContext::new(hir),
            config,
            oracle,
            lambda_units,
            handler_units,
            captures: BTreeMap::new(),
            semantic_captures: BTreeMap::new(),
            facts: AliasFacts::default(),
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        AliasFacts,
        Vec<Diagnostic>,
        BTreeMap<ContextualAliasUnit, BTreeMap<SymbolId, AliasValueExpr>>,
    ) {
        (self.facts, self.diagnostics, self.captures)
    }

    fn push_diagnostic(&mut self, span: Span, message: impl Into<String>) {
        let diagnostic =
            Diagnostic::analysis(AnalysisDiagnosticCode::MissingCheckedFact, span, message);
        if !self.diagnostics.contains(&diagnostic) {
            self.diagnostics.push(diagnostic);
        }
    }

    fn span_of_expr(&self, expr: HirExprId) -> Span {
        self.hir.exprs[expr].span(&self.hir.blocks)
    }

    fn allocation_for_unit(
        &mut self,
        unit: Option<HirSemanticUnit>,
        expr: HirExprId,
        kind: AllocationKind,
    ) -> AliasValue {
        if matches!(self.config.heap_model, HeapModel::NoHeap) {
            self.push_diagnostic(
                self.span_of_expr(expr),
                "alias heap model is disabled for allocation",
            );
            return AliasValue::unknown();
        }
        let target = self.oracle.allocation_site(expr, kind).unwrap_or_else(|| {
            match self.config.allocation_sensitivity {
                AllocationSensitivity::PerType => AliasTarget::AllocationKind {
                    unit: None,
                    kind: allocation_site_kind(kind),
                },
                AllocationSensitivity::PerExpression | AllocationSensitivity::PerCallSite => {
                    AliasTarget::Allocation { unit, expr }
                }
            }
        });
        AliasValue::from_target(target)
    }

    fn unknown_expr(
        &mut self,
        mut state: AliasDomain,
        expr: HirExprId,
        message: impl Into<String>,
    ) -> AliasDomain {
        self.push_diagnostic(self.span_of_expr(expr), message);
        state.mark_incomplete();
        state.set_expr_alias(expr, AliasValue::unknown());
        self.facts.record_expr(expr, AliasValue::unknown());
        state
    }

    fn solve_copy_update(
        config: AliasPrecisionConfig,
        dst: AliasVar,
        current: Option<&AliasValue>,
        incoming: &AliasValue,
        strong_update: bool,
    ) -> (AliasValue, bool) {
        let src = AliasVar::Synthetic(0);
        let mut frame = AliasConstraintFrame::new(config);
        let strong_update =
            strong_update && matches!(config.flow_sensitivity, FlowSensitivity::FlowSensitive);
        if !strong_update || matches!(config.update_policy, UpdatePolicy::AlwaysWeak) {
            if let Some(current) = current {
                frame.seed(dst.clone(), current.clone());
            }
        }
        frame.seed(src.clone(), incoming.clone());
        frame.add_constraint(AliasConstraint::Copy {
            dst: dst.clone(),
            src,
        });
        let solution = frame.solve();
        (solution.value(&dst), solution.incomplete)
    }

    fn solve_projection(
        config: AliasPrecisionConfig,
        base: AliasValue,
        projection: Projection,
    ) -> (AliasValue, bool) {
        solve_projection_value(config, base, projection)
    }

    fn solve_unknown_call(
        &self,
        state: &AliasDomain,
        call: HirExprId,
        callee: HirExprId,
    ) -> (AliasValue, bool) {
        let mut frame = AliasConstraintFrame::new(state.config);
        let callee_var = AliasVar::Expr(callee);
        frame.seed(callee_var.clone(), state.expr_alias(callee));
        let mut args = Vec::new();
        for (index, arg) in call_args(self.hir, call).into_iter().enumerate() {
            let var = AliasVar::Synthetic(index as u32);
            frame.seed(var.clone(), state.expr_alias(arg));
            args.push(var);
        }
        let dst = AliasVar::Expr(call);
        frame.add_constraint(AliasConstraint::Call {
            dst: Some(dst.clone()),
            callee: callee_var,
            args,
            site: call,
        });
        let solution = frame.solve();
        (solution.value(&dst), solution.incomplete)
    }

    fn assign_symbol_with_constraints(
        &mut self,
        state: &mut AliasDomain,
        symbol: SymbolId,
        value: AliasValue,
        strong_update: bool,
    ) -> AliasValue {
        let (solved, incomplete) = Self::solve_copy_update(
            state.config,
            AliasVar::Symbol(symbol),
            state.symbols.get(&symbol),
            &value,
            strong_update,
        );
        if incomplete {
            state.mark_incomplete();
        }
        state.assign_symbol(symbol, solved.clone());
        solved
    }

    fn assign_place_with_constraints(
        &mut self,
        state: &mut AliasDomain,
        place: Place,
        value: AliasValue,
        strong_update: bool,
    ) {
        let (solved, incomplete) = Self::solve_copy_update(
            state.config,
            AliasVar::Place(place.clone()),
            state.heap.get(&place),
            &value,
            strong_update,
        );
        if incomplete {
            state.mark_incomplete();
        }
        state.assign_place(place, solved);
    }

    fn path_alias(&mut self, state: &AliasDomain, expr: HirExprId) -> AliasValue {
        let HirExpr::Path(path) = &self.hir.exprs[expr] else {
            return AliasValue::bottom();
        };
        match path.resolution {
            ResolveResult::Resolved(symbol) => state
                .symbols
                .get(&symbol)
                .cloned()
                .or_else(|| {
                    self.oracle
                        .symbol_target(symbol)
                        .map(AliasValue::from_target)
                })
                .unwrap_or_else(AliasValue::unknown),
            ResolveResult::PartiallyResolved(_)
            | ResolveResult::Unresolved
            | ResolveResult::Ambiguous(_) => AliasValue::unknown(),
        }
    }

    fn bind_pattern(&mut self, state: &mut AliasDomain, pat: HirPatId, value: AliasValue) {
        let Some(pat_data) = self.hir.pats.get(pat).cloned() else {
            state.mark_incomplete();
            return;
        };
        match pat_data {
            HirPat::Binding { symbol, .. } => {
                let value = self.assign_symbol_with_constraints(state, symbol, value, true);
                self.facts.record_symbol(symbol, value);
            }
            HirPat::Tuple { elems, .. } => {
                for (index, elem) in elems.into_iter().enumerate() {
                    let (value, incomplete) = Self::solve_projection(
                        state.config,
                        value.clone(),
                        Projection::ConstIndex(index.to_string()),
                    );
                    if incomplete {
                        state.mark_incomplete();
                    }
                    self.bind_pattern(state, elem, value);
                }
            }
            HirPat::Record { fields, .. } => {
                for field in fields {
                    if let Some(pat) = field.pat {
                        let (value, incomplete) = Self::solve_projection(
                            state.config,
                            value.clone(),
                            field_projection(state.config, field.name),
                        );
                        if incomplete {
                            state.mark_incomplete();
                        }
                        self.bind_pattern(state, pat, value);
                    }
                }
            }
            HirPat::Variant { args, .. } => {
                for (index, arg) in args.into_iter().enumerate() {
                    let (value, incomplete) = Self::solve_projection(
                        state.config,
                        value.clone(),
                        Projection::ConstIndex(index.to_string()),
                    );
                    if incomplete {
                        state.mark_incomplete();
                    }
                    self.bind_pattern(state, arg, value);
                }
            }
            HirPat::Wildcard { .. } | HirPat::Literal(_) => {}
            HirPat::Error { .. } => state.mark_incomplete(),
        }
    }

    fn assign_target(&mut self, state: &mut AliasDomain, target: HirExprId, value: AliasValue) {
        match self.hir.exprs.get(target).cloned() {
            Some(HirExpr::Path(path)) => match path.resolution {
                ResolveResult::Resolved(symbol) => match state.config.update_policy {
                    super::config::UpdatePolicy::AlwaysWeak => {
                        let value = self.assign_symbol_with_constraints(
                            state,
                            symbol,
                            value.clone(),
                            false,
                        );
                        self.facts.record_symbol(symbol, value);
                    }
                    super::config::UpdatePolicy::StrongWhenMustAlias => {
                        let value =
                            self.assign_symbol_with_constraints(state, symbol, value.clone(), true);
                        self.facts.record_symbol(symbol, value);
                    }
                },
                _ => {
                    state.mark_incomplete();
                    self.push_diagnostic(path.span, "assignment target path is not resolved");
                }
            },
            Some(HirExpr::Field { .. }) | Some(HirExpr::Index { .. }) => {
                let target_alias = state.expr_alias(target);
                if let Some(place) = target_alias.must.clone() {
                    match state.config.update_policy {
                        super::config::UpdatePolicy::StrongWhenMustAlias => {
                            self.assign_place_with_constraints(state, place, value, true)
                        }
                        super::config::UpdatePolicy::AlwaysWeak => {
                            self.assign_place_with_constraints(state, place, value, false)
                        }
                    }
                } else {
                    for place in target_alias.may.places().cloned().unwrap_or_default() {
                        self.assign_place_with_constraints(state, place, value.clone(), false);
                    }
                    if target_alias.is_unknown() {
                        state.mark_incomplete();
                    }
                }
            }
            Some(_) | None => {
                state.mark_incomplete();
                self.push_diagnostic(self.span_of_expr(target), "unsupported assignment target");
            }
        }
    }

    fn apply_external_or_unknown(
        &mut self,
        mut state: AliasDomain,
        call: HirExprId,
        callee: HirExprId,
    ) -> AliasDomain {
        if let Some(target) = self.oracle.resource_constructor(call) {
            let value = AliasValue::from_target(target);
            state.set_expr_alias(call, value.clone());
            self.facts.record_expr(call, value);
            return state;
        }

        if let Some(summary) = self.oracle.intrinsic_call_summary(call, callee) {
            return self.apply_summary_to_call(call, None, &summary.summary, state);
        }

        let args = call_args(self.hir, call);
        for arg in args {
            let value = state.expr_alias(arg);
            state.mark_escaped(&value);
        }
        let (value, incomplete) = self.solve_unknown_call(&state, call, callee);
        if incomplete {
            state.mark_incomplete();
        }
        let mut state = self.unknown_expr(state, call, "call target has no alias summary");
        state.set_expr_alias(call, value.clone());
        self.facts.record_expr(call, value);
        state
    }

    fn apply_summary_to_call(
        &mut self,
        call: HirExprId,
        callee: Option<ContextualAliasUnit>,
        summary: &AliasSummary,
        mut state: AliasDomain,
    ) -> AliasDomain {
        let actuals = call_args(self.hir, call)
            .into_iter()
            .map(|expr| state.expr_alias(expr))
            .collect::<Vec<_>>();
        let captures = self.captures_for_call(callee, summary);

        let value = instantiate_value_at_call(
            call,
            state.unit,
            &summary.return_alias,
            &actuals,
            &captures,
            state.config,
        );
        state.set_expr_alias(call, value.clone());
        self.facts.record_expr(call, value);

        for escaped in &summary.escaped {
            let value = instantiate_value_at_call(
                call,
                state.unit,
                escaped,
                &actuals,
                &captures,
                state.config,
            );
            state.mark_escaped(&value);
        }
        for write in &summary.writes {
            let target = instantiate_value_at_call(
                call,
                state.unit,
                &write.target,
                &actuals,
                &captures,
                state.config,
            );
            let value = instantiate_value_at_call(
                call,
                state.unit,
                &write.value,
                &actuals,
                &captures,
                state.config,
            );
            if let Some(place) = target.must {
                self.assign_place_with_constraints(&mut state, place, value, false);
            } else if target.is_unknown() {
                state.mark_incomplete();
            }
        }
        if summary.incomplete {
            state.mark_incomplete();
        }
        state
    }

    fn captures_for_call(
        &self,
        callee: Option<ContextualAliasUnit>,
        summary: &AliasSummary,
    ) -> BTreeMap<SymbolId, AliasValueExpr> {
        if let Some(callee) = callee {
            if let Some(captures) = self.captures.get(&callee) {
                return captures.clone();
            }
            if let Some(captures) = self.semantic_captures.get(&callee.semantic) {
                return captures.clone();
            }
        }
        summary.captures.clone()
    }

    fn params_for_unit(&self, unit: HirSemanticUnit) -> Vec<SymbolId> {
        match unit {
            HirSemanticUnit::Item(item) => match self.hir.items.get(item) {
                Some(HirItem::Flow(flow)) => flow.params.clone(),
                Some(HirItem::Tool(tool)) => tool.params.clone(),
                Some(HirItem::Agent(agent)) => agent.params.clone(),
                _ => Vec::new(),
            },
            HirSemanticUnit::AnonymousFlow { expr, .. } => match self.hir.exprs.get(expr) {
                Some(HirExpr::Lambda { params, .. }) => params.clone(),
                _ => Vec::new(),
            },
            HirSemanticUnit::HandlerArm(_) | HirSemanticUnit::TopLevelLet(_) => Vec::new(),
        }
    }

    fn capture_symbols_for_unit(&self, unit: HirSemanticUnit) -> BTreeSet<SymbolId> {
        let mut locals = self
            .params_for_unit(unit)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut captured = BTreeSet::new();
        match unit {
            HirSemanticUnit::AnonymousFlow { expr, .. } => {
                if let Some(HirExpr::Lambda { params, body, .. }) = self.hir.exprs.get(expr) {
                    locals.extend(params.iter().copied());
                    match *body {
                        etas_hir::HirLambdaBody::Expr(expr) => {
                            self.collect_expr_captures(expr, &mut locals, &mut captured)
                        }
                        etas_hir::HirLambdaBody::Block(block) => {
                            self.collect_block_captures(block, &mut locals, &mut captured)
                        }
                    }
                }
            }
            HirSemanticUnit::HandlerArm(arm) => {
                if let Some(arm_data) = self.hir.handler_arms.get(arm) {
                    for pat in &arm_data.patterns {
                        self.collect_pattern_bindings(*pat, &mut locals);
                    }
                    self.collect_block_captures(arm_data.body, &mut locals, &mut captured);
                }
            }
            HirSemanticUnit::Item(_) | HirSemanticUnit::TopLevelLet(_) => {}
        }
        captured
    }

    fn record_lambda_captures(&mut self, state: &AliasDomain, expr: HirExprId) {
        let Some(unit) = self.lambda_units.get(&expr).copied() else {
            return;
        };
        let Some(HirExpr::Lambda { params, body, .. }) = self.hir.exprs.get(expr).cloned() else {
            return;
        };
        let mut locals = params.into_iter().collect::<BTreeSet<_>>();
        let mut captured = BTreeSet::new();
        match body {
            etas_hir::HirLambdaBody::Expr(expr) => {
                self.collect_expr_captures(expr, &mut locals, &mut captured)
            }
            etas_hir::HirLambdaBody::Block(block) => {
                self.collect_block_captures(block, &mut locals, &mut captured)
            }
        }
        self.record_capture_values(state, unit, captured);
    }

    fn record_handler_captures(&mut self, state: &AliasDomain, expr: HirExprId) {
        let Some(HirExpr::Handler { handlers, .. }) = self.hir.exprs.get(expr).cloned() else {
            return;
        };
        for arm in handlers {
            self.record_handler_arm_captures(state, arm);
        }
    }

    fn record_handler_arm_captures(&mut self, state: &AliasDomain, arm: HirHandlerArmId) {
        let Some(unit) = self.handler_units.get(&arm).copied() else {
            return;
        };
        let Some(arm_data) = self.hir.handler_arms.get(arm).cloned() else {
            return;
        };
        let mut locals = BTreeSet::new();
        for pat in arm_data.patterns {
            self.collect_pattern_bindings(pat, &mut locals);
        }
        let mut captured = BTreeSet::new();
        self.collect_block_captures(arm_data.body, &mut locals, &mut captured);
        self.record_capture_values(state, unit, captured);
    }

    fn record_capture_values(
        &mut self,
        state: &AliasDomain,
        unit: HirSemanticUnit,
        captured: BTreeSet<SymbolId>,
    ) {
        let Some(current_unit) = state.unit else {
            return;
        };
        let context = state.context;
        let contextual_unit = ContextualAliasUnit::new(unit, context);
        let entries = self.captures.entry(contextual_unit).or_default();
        let semantic_entries = self.semantic_captures.entry(unit).or_default();
        for symbol in captured {
            let value = if let Some(value) = state.symbols.get(&symbol) {
                AliasSummary::value_to_expr(value, current_unit)
            } else if let Some(target) = self.oracle.symbol_target(symbol) {
                AliasSummary::value_to_expr(&AliasValue::from_target(target), current_unit)
            } else {
                AliasValueExpr::Unknown
            };
            entries.insert(symbol, value.clone());
            semantic_entries.insert(symbol, value);
        }
    }

    fn collect_block_captures(
        &self,
        block: etas_hir::HirBlockId,
        locals: &mut BTreeSet<SymbolId>,
        captured: &mut BTreeSet<SymbolId>,
    ) {
        let Some(block_data) = self.hir.blocks.get(block).cloned() else {
            return;
        };
        for stmt in block_data.stmts {
            self.collect_stmt_captures(stmt, locals, captured);
        }
        if let Some(expr) = block_data.final_expr {
            self.collect_expr_captures(expr, locals, captured);
        }
    }

    fn collect_stmt_captures(
        &self,
        stmt: HirStmtId,
        locals: &mut BTreeSet<SymbolId>,
        captured: &mut BTreeSet<SymbolId>,
    ) {
        let Some(stmt_data) = self.hir.stmts.get(stmt).cloned() else {
            return;
        };
        match stmt_data {
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                self.collect_expr_captures(value, locals, captured);
                self.collect_pattern_bindings(pat, locals);
            }
            HirStmt::Assign { target, value, .. } => {
                self.collect_expr_captures(target, locals, captured);
                self.collect_expr_captures(value, locals, captured);
            }
            HirStmt::Expr { expr, .. }
            | HirStmt::If(expr)
            | HirStmt::Match(expr)
            | HirStmt::Return {
                value: Some(expr), ..
            }
            | HirStmt::Resume {
                value: Some(expr), ..
            }
            | HirStmt::Finish { value: expr, .. } => {
                self.collect_expr_captures(expr, locals, captured)
            }
            HirStmt::For {
                iter, limits, body, ..
            }
            | HirStmt::While {
                cond: iter,
                limits,
                body,
                ..
            } => {
                self.collect_expr_captures(iter, locals, captured);
                for limit in limits {
                    self.collect_expr_captures(limit, locals, captured);
                }
                self.collect_block_captures(body, locals, captured);
            }
            HirStmt::Retry { limits, body, .. } => {
                for limit in limits {
                    self.collect_expr_captures(limit, locals, captured);
                }
                self.collect_block_captures(body, locals, captured);
            }
            HirStmt::Return { value: None, .. }
            | HirStmt::Resume { value: None, .. }
            | HirStmt::Break { .. }
            | HirStmt::Continue { .. }
            | HirStmt::Error { .. } => {}
        }
    }

    fn collect_expr_captures(
        &self,
        expr: HirExprId,
        locals: &mut BTreeSet<SymbolId>,
        captured: &mut BTreeSet<SymbolId>,
    ) {
        let Some(expr_data) = self.hir.exprs.get(expr).cloned() else {
            return;
        };
        match expr_data {
            HirExpr::Path(path) => {
                if let ResolveResult::Resolved(symbol) = path.resolution {
                    if !locals.contains(&symbol) {
                        captured.insert(symbol);
                    }
                }
            }
            HirExpr::Record(record) => {
                for field in record.fields {
                    if let HirFieldInit::Named { value, .. } = field {
                        self.collect_expr_captures(value, locals, captured);
                    }
                }
            }
            HirExpr::Tuple { elems, .. }
            | HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Set { elems, .. } => {
                for elem in elems {
                    self.collect_expr_captures(elem, locals, captured);
                }
            }
            HirExpr::Map { entries, .. } => {
                for entry in entries {
                    self.collect_expr_captures(entry.key, locals, captured);
                    self.collect_expr_captures(entry.value, locals, captured);
                }
            }
            HirExpr::Call { callee, args, .. } => {
                self.collect_expr_captures(callee, locals, captured);
                self.collect_arg_captures(&args, locals, captured);
            }
            HirExpr::MethodCall { receiver, args, .. } => {
                self.collect_expr_captures(receiver, locals, captured);
                self.collect_arg_captures(&args, locals, captured);
            }
            HirExpr::SpecMethodCall { receiver, args, .. } => {
                self.collect_expr_captures(receiver, locals, captured);
                self.collect_arg_captures(&args, locals, captured);
            }
            HirExpr::Perform { args, .. } => self.collect_arg_captures(&args, locals, captured),
            HirExpr::StageCompose { stages, .. } => {
                for stage in stages {
                    self.collect_expr_captures(stage.expr, locals, captured);
                    for limit in stage.limits {
                        self.collect_expr_captures(limit, locals, captured);
                    }
                }
            }
            HirExpr::Pipeline { input, stages, .. } => {
                self.collect_expr_captures(input, locals, captured);
                for stage in stages {
                    self.collect_expr_captures(stage.expr, locals, captured);
                    for limit in stage.limits {
                        self.collect_expr_captures(limit, locals, captured);
                    }
                }
            }
            HirExpr::Field { base, .. } | HirExpr::Try { expr: base, .. } => {
                self.collect_expr_captures(base, locals, captured);
            }
            HirExpr::Index { base, index, .. } => {
                self.collect_expr_captures(base, locals, captured);
                self.collect_expr_captures(index, locals, captured);
            }
            HirExpr::Slice {
                base, start, end, ..
            } => {
                self.collect_expr_captures(base, locals, captured);
                self.collect_expr_captures(start, locals, captured);
                self.collect_expr_captures(end, locals, captured);
            }
            HirExpr::Unary { expr, .. } => self.collect_expr_captures(expr, locals, captured),
            HirExpr::Binary { lhs, rhs, .. }
            | HirExpr::Range {
                start: lhs,
                end: rhs,
                ..
            } => {
                self.collect_expr_captures(lhs, locals, captured);
                self.collect_expr_captures(rhs, locals, captured);
            }
            HirExpr::If {
                cond,
                then_block,
                else_branch,
                ..
            } => {
                self.collect_expr_captures(cond, locals, captured);
                self.collect_block_captures(then_block, locals, captured);
                match else_branch {
                    Some(etas_hir::HirElseBranch::If(expr)) => {
                        self.collect_expr_captures(expr, locals, captured)
                    }
                    Some(etas_hir::HirElseBranch::Block(block)) => {
                        self.collect_block_captures(block, locals, captured)
                    }
                    None => {}
                }
            }
            HirExpr::Match {
                scrutinee, arms, ..
            } => {
                self.collect_expr_captures(scrutinee, locals, captured);
                for arm in arms {
                    match arm.body {
                        etas_hir::HirMatchArmBody::Expr(expr) => {
                            self.collect_expr_captures(expr, locals, captured)
                        }
                        etas_hir::HirMatchArmBody::Block(block) => {
                            self.collect_block_captures(block, locals, captured)
                        }
                    }
                }
            }
            HirExpr::Block(block) => self.collect_block_captures(block, locals, captured),
            HirExpr::ListCons { head, tail, .. } => {
                self.collect_expr_captures(head, locals, captured);
                self.collect_expr_captures(tail, locals, captured);
            }
            HirExpr::Handle { body, handler, .. } => {
                self.collect_expr_captures(body, locals, captured);
                self.collect_expr_captures(handler, locals, captured);
            }
            HirExpr::Lambda { .. } | HirExpr::Handler { .. } => {}
            HirExpr::Literal(_)
            | HirExpr::EmptyRecordOrMap { .. }
            | HirExpr::EmptySequence { .. }
            | HirExpr::Error { .. } => {}
        }
    }

    fn collect_arg_captures(
        &self,
        args: &[HirArg],
        locals: &mut BTreeSet<SymbolId>,
        captured: &mut BTreeSet<SymbolId>,
    ) {
        for arg in args {
            let expr = match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            };
            self.collect_expr_captures(expr, locals, captured);
        }
    }

    fn collect_pattern_bindings(&self, pat: HirPatId, locals: &mut BTreeSet<SymbolId>) {
        let Some(pat_data) = self.hir.pats.get(pat).cloned() else {
            return;
        };
        match pat_data {
            HirPat::Binding { symbol, .. } => {
                locals.insert(symbol);
            }
            HirPat::Tuple { elems, .. } => {
                for elem in elems {
                    self.collect_pattern_bindings(elem, locals);
                }
            }
            HirPat::Record { fields, .. } => {
                for field in fields {
                    if let Some(pat) = field.pat {
                        self.collect_pattern_bindings(pat, locals);
                    }
                }
            }
            HirPat::Variant { args, .. } => {
                for arg in args {
                    self.collect_pattern_bindings(arg, locals);
                }
            }
            HirPat::Wildcard { .. } | HirPat::Literal(_) | HirPat::Error { .. } => {}
        }
    }

    fn finish_expr_alias(&mut self, expr: HirExprId, mut state: AliasDomain) -> AliasDomain {
        let expr_data = self.hir.exprs[expr].clone();
        let alias = match expr_data {
            HirExpr::Literal(_)
            | HirExpr::EmptyRecordOrMap { .. }
            | HirExpr::EmptySequence { .. } => AliasValue::bottom(),
            HirExpr::Path(_) => {
                let value = self.path_alias(&state, expr);
                if value.is_unknown() {
                    self.push_diagnostic(
                        self.span_of_expr(expr),
                        "path alias target is unresolved",
                    );
                    state.mark_incomplete();
                }
                value
            }
            HirExpr::Record(record) => {
                let value = self.allocation_for_unit(state.unit, expr, AllocationKind::Record);
                if let Some(root) = value.must.clone() {
                    for field in record.fields {
                        match field {
                            HirFieldInit::Named { name, value, .. } => {
                                let field_value = state.expr_alias(value);
                                if let Some(place) = root.project(
                                    field_projection(state.config, name),
                                    state.config.max_projection_depth,
                                ) {
                                    let strong = state.config.field_sensitivity
                                        != FieldSensitivity::Collapsed;
                                    self.assign_place_with_constraints(
                                        &mut state,
                                        place,
                                        field_value,
                                        strong,
                                    );
                                } else {
                                    state.mark_incomplete();
                                }
                            }
                            HirFieldInit::Shorthand {
                                name, resolution, ..
                            } => {
                                if let ResolveResult::Resolved(symbol) = resolution {
                                    let field_value = state
                                        .symbols
                                        .get(&symbol)
                                        .cloned()
                                        .unwrap_or_else(AliasValue::unknown);
                                    if let Some(place) = root.project(
                                        field_projection(state.config, name),
                                        state.config.max_projection_depth,
                                    ) {
                                        let strong = state.config.field_sensitivity
                                            != FieldSensitivity::Collapsed;
                                        self.assign_place_with_constraints(
                                            &mut state,
                                            place,
                                            field_value,
                                            strong,
                                        );
                                    } else {
                                        state.mark_incomplete();
                                    }
                                } else {
                                    state.mark_incomplete();
                                }
                            }
                        }
                    }
                }
                value
            }
            HirExpr::Tuple { elems, .. } => {
                self.sequence_allocation(&mut state, expr, AllocationKind::Tuple, elems)
            }
            HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Set { elems, .. } => {
                self.sequence_allocation(&mut state, expr, AllocationKind::Array, elems)
            }
            HirExpr::Map { entries, .. } => {
                let value = self.allocation_for_unit(state.unit, expr, AllocationKind::Map);
                if let Some(root) = value.must.clone() {
                    for entry in entries {
                        let projection = map_projection(self.hir, entry.key, state.config);
                        let entry_value = state.expr_alias(entry.value);
                        if let Some(place) =
                            root.project(projection, state.config.max_projection_depth)
                        {
                            self.assign_place_with_constraints(
                                &mut state,
                                place,
                                entry_value,
                                false,
                            );
                        } else {
                            state.mark_incomplete();
                        }
                    }
                }
                value
            }
            HirExpr::Range { .. } => {
                self.allocation_for_unit(state.unit, expr, AllocationKind::Range)
            }
            HirExpr::Field { base, field, .. } => {
                let (value, incomplete) = project_read(
                    &state,
                    state.expr_alias(base),
                    field_projection(state.config, field),
                );
                if incomplete {
                    state.mark_incomplete();
                }
                value
            }
            HirExpr::Index { base, index, .. } => {
                let projection = index_projection(self.hir, index, state.config);
                let (value, incomplete) = project_read(&state, state.expr_alias(base), projection);
                if incomplete {
                    state.mark_incomplete();
                }
                value
            }
            HirExpr::Slice { base, .. } => {
                let (value, incomplete) =
                    project_read(&state, state.expr_alias(base), Projection::Index);
                if incomplete {
                    state.mark_incomplete();
                }
                value
            }
            HirExpr::Lambda { .. } => {
                self.record_lambda_captures(&state, expr);
                self.allocation_for_unit(state.unit, expr, AllocationKind::Lambda)
            }
            HirExpr::Handler { .. } => {
                self.record_handler_captures(&state, expr);
                self.allocation_for_unit(state.unit, expr, AllocationKind::Handler)
            }
            HirExpr::Block(_)
            | HirExpr::If { .. }
            | HirExpr::Match { .. }
            | HirExpr::Try { .. }
            | HirExpr::Unary { .. }
            | HirExpr::Binary { .. }
            | HirExpr::Handle { .. }
            | HirExpr::StageCompose { .. }
            | HirExpr::Pipeline { .. }
            | HirExpr::ListCons { .. } => state.last_expr_alias.clone(),
            HirExpr::Call { .. }
            | HirExpr::MethodCall { .. }
            | HirExpr::SpecMethodCall { .. }
            | HirExpr::Perform { .. } => state.expr_alias(expr),
            HirExpr::Error { .. } => {
                state.mark_incomplete();
                AliasValue::unknown()
            }
        };
        if alias.is_unknown() {
            state.mark_incomplete();
        }
        state.set_expr_alias(expr, alias.clone());
        self.facts.record_expr(expr, alias);
        state
    }

    fn sequence_allocation(
        &mut self,
        state: &mut AliasDomain,
        expr: HirExprId,
        kind: AllocationKind,
        elems: Vec<HirExprId>,
    ) -> AliasValue {
        let value = self.allocation_for_unit(state.unit, expr, kind);
        if let Some(root) = value.must.clone() {
            for (index, elem) in elems.into_iter().enumerate() {
                let projection = match state.config.index_sensitivity {
                    super::config::IndexSensitivity::Collapsed => Projection::Index,
                    super::config::IndexSensitivity::ConstantIndex
                    | super::config::IndexSensitivity::KeySensitive => {
                        Projection::ConstIndex(index.to_string())
                    }
                };
                let elem_value = state.expr_alias(elem);
                if let Some(place) = root.project(projection, state.config.max_projection_depth) {
                    self.assign_place_with_constraints(state, place, elem_value, false);
                } else {
                    state.mark_incomplete();
                }
            }
        }
        value
    }
}

impl<O> HirAnalysisSemantics for AliasSemantics<'_, O>
where
    O: AliasOracle,
{
    type Domain = AliasDomain;

    fn hir(&self) -> &HirProgram {
        self.hir
    }

    fn incomplete_facts(&mut self, mut state: Self::Domain) -> Self::Domain {
        state.mark_incomplete();
        state
    }

    fn after_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        self.finish_expr_alias(expr, state)
    }

    fn after_stmt(&mut self, stmt: HirStmtId, mut state: Self::Domain) -> Self::Domain {
        let Some(stmt_data) = self.hir.stmts.get(stmt).cloned() else {
            state.mark_incomplete();
            return state;
        };
        match stmt_data {
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                let value_alias = state.expr_alias(value);
                self.bind_pattern(&mut state, pat, value_alias);
            }
            HirStmt::Assign { target, value, .. } => {
                let value_alias = state.expr_alias(value);
                self.assign_target(&mut state, target, value_alias);
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    let value_alias = state.expr_alias(value);
                    let limit = state.config.max_alias_set_size;
                    state.return_alias.join_with_limit(&value_alias, limit);
                }
            }
            HirStmt::Resume { value, .. } => {
                if let Some(value) = value {
                    let value_alias = state.expr_alias(value);
                    state.mark_escaped(&value_alias);
                }
            }
            HirStmt::Finish { value, .. } => {
                let value_alias = state.expr_alias(value);
                let limit = state.config.max_alias_set_size;
                state.return_alias.join_with_limit(&value_alias, limit);
            }
            HirStmt::If(_)
            | HirStmt::Match(_)
            | HirStmt::For { .. }
            | HirStmt::While { .. }
            | HirStmt::Retry { .. }
            | HirStmt::Break { .. }
            | HirStmt::Continue { .. }
            | HirStmt::Expr { .. }
            | HirStmt::Error { .. } => {}
        }
        state
    }

    fn perform(
        &mut self,
        expr: HirExprId,
        _action: &ResolvedActionRef,
        _generic_args: &[HirGenericArg],
        args: &[HirArg],
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        for arg in args {
            let value = match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => {
                    state.expr_alias(*expr)
                }
            };
            state.mark_escaped(&value);
        }
        state = self.unknown_expr(state, expr, "perform has no alias oracle summary");
        AnalysisStep::handled(state)
    }

    fn direct_call(
        &mut self,
        _call: HirExprId,
        _callee: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn method_call(
        &mut self,
        expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let callee = match self.hir.exprs.get(expr) {
            Some(HirExpr::MethodCall { receiver, .. }) => *receiver,
            _ => expr,
        };
        AnalysisStep::handled(self.apply_external_or_unknown(state, expr, callee))
    }

    fn stage_compose(
        &mut self,
        expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.unknown_expr(
            state,
            expr,
            "stage composition aliases are unknown",
        ))
    }

    fn pipeline(
        &mut self,
        expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.unknown_expr(state, expr, "pipeline aliases are unknown"))
    }

    fn try_expr(
        &mut self,
        expr: HirExprId,
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if let Some(HirExpr::Try { expr: operand, .. }) = self.hir.exprs.get(expr) {
            let value = state.expr_alias(*operand);
            state.set_expr_alias(expr, value.clone());
            self.facts.record_expr(expr, value);
        }
        AnalysisStep::handled(state)
    }

    fn handle_expr(
        &mut self,
        expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.unknown_expr(
            state,
            expr,
            "handler application aliases are unknown",
        ))
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        self.record_lambda_captures(&state, expr);
        let value = self.allocation_for_unit(state.unit, expr, AllocationKind::Lambda);
        state.set_expr_alias(expr, value.clone());
        self.facts.record_expr(expr, value);
        AnalysisStep::handled(state)
    }

    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        self.record_handler_captures(&state, expr);
        let value = self.allocation_for_unit(state.unit, expr, AllocationKind::Handler);
        state.set_expr_alias(expr, value.clone());
        self.facts.record_expr(expr, value);
        AnalysisStep::handled(state)
    }
}

impl<O> InterproceduralSemantics for AliasSemantics<'_, O>
where
    O: AliasOracle,
{
    type Unit = ContextualAliasUnit;
    type Domain = AliasDomain;
    type Summary = AliasSummary;

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody {
        unit.semantic
            .analysis_body_with_context(self.hir, &self.context)
    }

    fn begin_unit(&mut self, context: UnitContext<Self::Unit>) -> Self::Domain {
        let mut state = AliasDomain::new(self.config);
        state.unit = Some(context.unit.semantic);
        state.context = context.unit.context;
        for (index, symbol) in self
            .params_for_unit(context.unit.semantic)
            .into_iter()
            .enumerate()
        {
            let value = AliasValue::from_target(AliasTarget::Param {
                unit: context.unit.semantic,
                index,
            });
            state.assign_symbol(symbol, value.clone());
            self.facts.record_symbol(symbol, value);
        }
        for symbol in self.capture_symbols_for_unit(context.unit.semantic) {
            let value = AliasValue::from_target(AliasTarget::Captured {
                unit: context.unit.semantic,
                symbol,
            });
            state.assign_symbol(symbol, value);
        }
        state
    }

    fn end_unit(
        &mut self,
        context: UnitContext<Self::Unit>,
        exit: Control<Self::Domain>,
    ) -> Self::Summary {
        let mut joined = exit.clone().into_joined_domain();
        let mut summary = AliasSummary::new(context.unit.semantic, self.config);
        summary.params = self
            .params_for_unit(context.unit.semantic)
            .into_iter()
            .map(|_| Default::default())
            .collect();
        if let Some(normal) = exit.normal_state() {
            joined
                .return_alias
                .join_with_limit(&normal.last_expr_alias, self.config.max_alias_set_size);
        }
        for place in &joined.escaped {
            let expr = AliasSummary::value_to_expr(
                &AliasValue::from_place(place.clone()),
                context.unit.semantic,
            );
            if let AliasValueExpr::FormalParam(index) = expr {
                if let Some(param) = summary.params.get_mut(index) {
                    param.escaped = true;
                }
            }
            summary.escaped.insert(expr);
        }
        for (place, value) in &joined.heap {
            summary.writes.push(AliasWriteEffect {
                target: AliasSummary::value_to_expr(
                    &AliasValue::from_place(place.clone()),
                    context.unit.semantic,
                ),
                value: AliasSummary::value_to_expr(value, context.unit.semantic),
            });
        }
        if let Some(captures) = self.captures.get(&context.unit) {
            for (symbol, value) in captures {
                summary.captures.insert(*symbol, value.clone());
            }
        }
        summary.return_alias =
            AliasSummary::value_to_expr(&joined.return_alias, context.unit.semantic);
        summary.incomplete = joined.incomplete;
        self.facts
            .record_summary(context.unit.semantic, summary.clone());
        summary
    }

    fn stabilize_summary(
        &mut self,
        _context: UnitContext<Self::Unit>,
        mut summary: Self::Summary,
        _current: Option<&Self::Summary>,
    ) -> Self::Summary {
        if summary.escaped.len() > self.config.max_alias_set_size
            || summary.writes.len() > self.config.max_alias_set_size
        {
            summary.incomplete = true;
            summary.return_alias = summary
                .return_alias
                .join(&AliasValueExpr::Unknown, self.config.max_alias_set_size);
        }
        summary
    }

    fn external_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        AliasSummary::unknown(context.unit.semantic, self.config)
    }

    fn missing_summary(&mut self, context: UnitContext<Self::Unit>) -> Self::Summary {
        AliasSummary::unknown(context.unit.semantic, self.config)
    }

    fn call_target(
        &mut self,
        context: UnitContext<Self::Unit>,
        call: HirExprId,
        callee: HirExprId,
        _state: &Self::Domain,
    ) -> CallTarget<Self::Unit> {
        match self.oracle.resolved_call_target(call, callee) {
            AliasCallTarget::Direct(unit) => CallTarget::Direct(ContextualAliasUnit::new(
                unit,
                call_context(context.unit, call, self.config),
            )),
            AliasCallTarget::Dynamic => CallTarget::Dynamic,
            AliasCallTarget::External => CallTarget::External,
            AliasCallTarget::Incomplete => CallTarget::Incomplete,
        }
    }

    fn handler_arm_dependency(
        &mut self,
        context: UnitContext<Self::Unit>,
        arm: HirHandlerArmId,
    ) -> Option<Self::Unit> {
        self.handler_units
            .get(&arm)
            .copied()
            .map(|unit| ContextualAliasUnit::new(unit, context.unit.context))
    }

    fn anonymous_flow_dependency(
        &mut self,
        context: UnitContext<Self::Unit>,
        expr: HirExprId,
    ) -> Option<Self::Unit> {
        self.lambda_units
            .get(&expr)
            .copied()
            .map(|unit| ContextualAliasUnit::new(unit, context.unit.context))
    }

    fn direct_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        callee: Self::Unit,
        summary: &Self::Summary,
        state: Self::Domain,
    ) -> Self::Domain {
        self.apply_summary_to_call(site.call, Some(callee), summary, state)
    }

    fn dynamic_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.apply_external_or_unknown(state, site.call, site.callee_expr)
    }

    fn external_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.apply_external_or_unknown(state, site.call, site.callee_expr)
    }

    fn incomplete_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.unknown_expr(state, site.call, "call target is incomplete")
    }
}

fn call_args(hir: &HirProgram, call: HirExprId) -> Vec<HirExprId> {
    match hir.exprs.get(call) {
        Some(HirExpr::Call { args, .. })
        | Some(HirExpr::MethodCall { args, .. })
        | Some(HirExpr::SpecMethodCall { args, .. }) => args
            .iter()
            .map(|arg| match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn call_context(
    unit: ContextualAliasUnit,
    call: HirExprId,
    config: AliasPrecisionConfig,
) -> AliasContext {
    match config.context_sensitivity {
        super::config::ContextSensitivity::ContextInsensitive => AliasContext::empty(),
        super::config::ContextSensitivity::CallString { k } => unit.context.push(call, k),
    }
}

fn instantiate_value_at_call(
    call: HirExprId,
    current_unit: Option<HirSemanticUnit>,
    value: &AliasValueExpr,
    actuals: &[AliasValue],
    captures: &BTreeMap<SymbolId, AliasValueExpr>,
    config: AliasPrecisionConfig,
) -> AliasValue {
    match value {
        AliasValueExpr::Bottom => AliasValue::bottom(),
        AliasValueExpr::Unknown => AliasValue::unknown(),
        AliasValueExpr::FormalParam(index) => actuals
            .get(*index)
            .cloned()
            .unwrap_or_else(AliasValue::unknown),
        AliasValueExpr::Captured(symbol) => captures
            .get(symbol)
            .map(|value| instantiate_captured_value(current_unit, value, captures, config))
            .unwrap_or_else(AliasValue::unknown),
        AliasValueExpr::Allocation { unit, expr } => {
            let (unit, expr) = match config.allocation_sensitivity {
                AllocationSensitivity::PerCallSite if call != HirExprId(u32::MAX) => {
                    (current_unit, call)
                }
                AllocationSensitivity::PerType
                | AllocationSensitivity::PerExpression
                | AllocationSensitivity::PerCallSite => (*unit, *expr),
            };
            AliasValue::from_target(AliasTarget::Allocation { unit, expr })
        }
        AliasValueExpr::AllocationKind { unit, kind } => {
            AliasValue::from_target(AliasTarget::AllocationKind {
                unit: *unit,
                kind: *kind,
            })
        }
        AliasValueExpr::TopLevel(item) => AliasValue::from_target(AliasTarget::TopLevel(*item)),
        AliasValueExpr::ResourceHandle(symbol) => {
            AliasValue::from_target(AliasTarget::ResourceHandle(*symbol))
        }
        AliasValueExpr::MemoryPlace(path) => {
            AliasValue::from_target(AliasTarget::MemoryPlace(path.clone()))
        }
        AliasValueExpr::Project { base, projection } => {
            let (value, _) = solve_projection_value(
                config,
                instantiate_value_at_call(call, current_unit, base, actuals, captures, config),
                projection.clone(),
            );
            value
        }
        AliasValueExpr::Union(values) => {
            let mut joined = AliasValue::bottom();
            for value in values {
                joined.join_with_limit(
                    &instantiate_value_at_call(
                        call,
                        current_unit,
                        value,
                        actuals,
                        captures,
                        config,
                    ),
                    config.max_alias_set_size,
                );
            }
            joined
        }
    }
}

fn instantiate_captured_value(
    current_unit: Option<HirSemanticUnit>,
    value: &AliasValueExpr,
    captures: &BTreeMap<SymbolId, AliasValueExpr>,
    config: AliasPrecisionConfig,
) -> AliasValue {
    match value {
        AliasValueExpr::FormalParam(index) => current_unit
            .map(|unit| {
                AliasValue::from_target(AliasTarget::Param {
                    unit,
                    index: *index,
                })
            })
            .unwrap_or_else(AliasValue::unknown),
        AliasValueExpr::Captured(symbol) => captures
            .get(symbol)
            .map(|value| instantiate_captured_value(current_unit, value, captures, config))
            .unwrap_or_else(AliasValue::unknown),
        AliasValueExpr::Project { base, projection } => {
            let (value, _) = solve_projection_value(
                config,
                instantiate_captured_value(current_unit, base, captures, config),
                projection.clone(),
            );
            value
        }
        AliasValueExpr::Union(values) => {
            let mut joined = AliasValue::bottom();
            for value in values {
                joined.join_with_limit(
                    &instantiate_captured_value(current_unit, value, captures, config),
                    config.max_alias_set_size,
                );
            }
            joined
        }
        other => instantiate_value_at_call(
            HirExprId(u32::MAX),
            current_unit,
            other,
            &[],
            captures,
            config,
        ),
    }
}

fn index_projection(
    hir: &HirProgram,
    index: HirExprId,
    config: AliasPrecisionConfig,
) -> Projection {
    match config.index_sensitivity {
        super::config::IndexSensitivity::Collapsed => Projection::Index,
        super::config::IndexSensitivity::ConstantIndex
        | super::config::IndexSensitivity::KeySensitive => match hir.exprs.get(index) {
            Some(HirExpr::Literal(etas_hir::HirLiteral::Int { text, .. })) => {
                Projection::ConstIndex(text.clone())
            }
            Some(HirExpr::Literal(etas_hir::HirLiteral::String { value, .. }))
                if config.index_sensitivity == super::config::IndexSensitivity::KeySensitive =>
            {
                Projection::MapKey(value.clone())
            }
            _ => Projection::Index,
        },
    }
}

fn map_projection(hir: &HirProgram, key: HirExprId, config: AliasPrecisionConfig) -> Projection {
    if config.index_sensitivity == super::config::IndexSensitivity::KeySensitive {
        if let Some(HirExpr::Literal(etas_hir::HirLiteral::String { value, .. })) =
            hir.exprs.get(key)
        {
            return Projection::MapKey(value.clone());
        }
    }
    Projection::Index
}

fn field_projection(config: AliasPrecisionConfig, field: String) -> Projection {
    match config.field_sensitivity {
        FieldSensitivity::Collapsed => Projection::CollapsedField,
        FieldSensitivity::NamedFields | FieldSensitivity::FullProjection => {
            Projection::Field(field)
        }
    }
}

fn allocation_site_kind(kind: AllocationKind) -> AllocationSiteKind {
    match kind {
        AllocationKind::Record => AllocationSiteKind::Record,
        AllocationKind::Tuple => AllocationSiteKind::Tuple,
        AllocationKind::Array => AllocationSiteKind::Array,
        AllocationKind::List => AllocationSiteKind::List,
        AllocationKind::Map => AllocationSiteKind::Map,
        AllocationKind::Set => AllocationSiteKind::Set,
        AllocationKind::Range => AllocationSiteKind::Range,
        AllocationKind::Lambda => AllocationSiteKind::Lambda,
        AllocationKind::Handler => AllocationSiteKind::Handler,
        AllocationKind::Unknown => AllocationSiteKind::Unknown,
    }
}

fn solve_projection_value(
    config: AliasPrecisionConfig,
    base: AliasValue,
    projection: Projection,
) -> (AliasValue, bool) {
    let base_var = AliasVar::Synthetic(0);
    let dst = AliasVar::Synthetic(1);
    let mut frame = AliasConstraintFrame::new(config);
    frame.seed(base_var.clone(), base);
    frame.add_constraint(AliasConstraint::Project {
        dst: dst.clone(),
        base: base_var,
        projection,
    });
    let solution = frame.solve();
    (solution.value(&dst), solution.incomplete)
}

fn project_read(
    state: &AliasDomain,
    base: AliasValue,
    projection: Projection,
) -> (AliasValue, bool) {
    let (projected, incomplete) = solve_projection_value(state.config, base, projection);
    if let Some(place) = projected.must.as_ref() {
        if let Some(value) = state.heap.get(place) {
            return (value.clone(), incomplete);
        }
    }
    (projected, incomplete)
}
