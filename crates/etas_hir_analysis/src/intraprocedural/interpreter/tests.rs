use std::collections::{BTreeMap, BTreeSet};

use etas_core::{SourceId, Span, TextSize};
use etas_hir::{
    HirBlock, HirBlockId, HirElseBranch, HirExpr, HirExprId, HirFlowBody, HirFlowDecl, HirItem,
    HirLiteral, HirModule, HirPatId, HirProgram, HirStmt, HirStmtId, ScopeId, ScopeOwner, SymbolId,
};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::intraprocedural::{AnalysisStep, HirAbstractInterpreter, HirAnalysisSemantics};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EventDomain {
    events: BTreeSet<&'static str>,
    incomplete: bool,
    growth: usize,
}

impl EventDomain {
    fn initial() -> Self {
        Self::default()
    }

    fn with_event(mut self, event: &'static str) -> Self {
        self.events.insert(event);
        self
    }

    fn with_incomplete(mut self) -> Self {
        self.incomplete = true;
        self
    }
}

impl PartialOrder for EventDomain {
    fn less_equal(&self, other: &Self) -> bool {
        self.events.is_subset(&other.events)
            && (!self.incomplete || other.incomplete)
            && self.growth <= other.growth
    }
}

impl JoinSemiLattice for EventDomain {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        let old_len = self.events.len();
        let old_incomplete = self.incomplete;
        let old_growth = self.growth;
        self.events.extend(other.events.iter().copied());
        self.incomplete |= other.incomplete;
        self.growth = self.growth.max(other.growth);
        self.events.len() != old_len
            || self.incomplete != old_incomplete
            || self.growth != old_growth
    }
}

struct TraceSemantics {
    context: crate::HirAnalysisContext,
    expr_events: BTreeMap<HirExprId, &'static str>,
    after_expr_events: BTreeMap<HirExprId, &'static str>,
    before_stmt_events: BTreeMap<HirStmtId, &'static str>,
    after_stmt_events: BTreeMap<HirStmtId, &'static str>,
    incomplete_calls: BTreeSet<HirExprId>,
    handled_calls: BTreeMap<HirExprId, &'static str>,
    started_exprs: Vec<HirExprId>,
    completed_exprs: Vec<HirExprId>,
    started_stmts: Vec<HirStmtId>,
    completed_stmts: Vec<HirStmtId>,
    cancel_after_expr: Option<(HirExprId, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
    returned_values: Vec<(HirStmtId, Option<HirExprId>)>,
    terminal_exprs: BTreeSet<HirExprId>,
    match_entries: Vec<(HirExprId, HirExprId, HirPatId, usize)>,
    match_event: Option<&'static str>,
    symbolic_match_entries: Vec<(HirExprId, HirPatId, bool)>,
    growing_expr: Option<HirExprId>,
    incomplete_loops: Vec<HirBlockId>,
}

impl TraceSemantics {
    fn new(hir: HirProgram) -> Self {
        Self {
            context: crate::HirAnalysisContext::new(&etas_hir::HirRevision::try_new(hir).unwrap()),
            expr_events: BTreeMap::new(),
            after_expr_events: BTreeMap::new(),
            before_stmt_events: BTreeMap::new(),
            after_stmt_events: BTreeMap::new(),
            incomplete_calls: BTreeSet::new(),
            handled_calls: BTreeMap::new(),
            started_exprs: Vec::new(),
            completed_exprs: Vec::new(),
            started_stmts: Vec::new(),
            completed_stmts: Vec::new(),
            cancel_after_expr: None,
            returned_values: Vec::new(),
            terminal_exprs: BTreeSet::new(),
            match_entries: Vec::new(),
            match_event: None,
            symbolic_match_entries: Vec::new(),
            growing_expr: None,
            incomplete_loops: Vec::new(),
        }
    }

    fn event(mut self, expr: HirExprId, label: &'static str) -> Self {
        self.expr_events.insert(expr, label);
        self
    }

    fn before_stmt_event(mut self, stmt: HirStmtId, label: &'static str) -> Self {
        self.before_stmt_events.insert(stmt, label);
        self
    }

    fn after_stmt_event(mut self, stmt: HirStmtId, label: &'static str) -> Self {
        self.after_stmt_events.insert(stmt, label);
        self
    }

    fn incomplete_call(mut self, call: HirExprId) -> Self {
        self.incomplete_calls.insert(call);
        self
    }

    fn handled_call(mut self, call: HirExprId, label: &'static str) -> Self {
        self.handled_calls.insert(call, label);
        self
    }
}

impl HirAnalysisSemantics for TraceSemantics {
    type Domain = EventDomain;

    fn analysis_context(&self) -> &crate::HirAnalysisContext {
        &self.context
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        state.with_incomplete()
    }

    fn loop_fixpoint_incomplete(&mut self, body: HirBlockId, state: Self::Domain) -> Self::Domain {
        self.incomplete_loops.push(body);
        state.with_incomplete()
    }

    fn before_stmt(&mut self, stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        self.started_stmts.push(stmt);
        self.before_stmt_events
            .get(&stmt)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
    }

    fn after_stmt(&mut self, stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        self.completed_stmts.push(stmt);
        self.after_stmt_events
            .get(&stmt)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
    }

    fn before_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        self.started_exprs.push(expr);
        let mut state = state;
        if self.growing_expr == Some(expr) {
            state.growth += 1;
        }
        self.expr_events
            .get(&expr)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
    }

    fn after_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        self.completed_exprs.push(expr);
        if let Some((target, signal)) = &self.cancel_after_expr
            && expr == *target
        {
            signal.store(true, std::sync::atomic::Ordering::Release);
        }
        self.after_expr_events
            .get(&expr)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
    }

    fn begin_match_arm(
        &mut self,
        expr: HirExprId,
        scrutinee: HirExprId,
        pattern: HirPatId,
        state: Self::Domain,
    ) -> Self::Domain {
        assert!(self.completed_exprs.contains(&scrutinee));
        self.match_entries
            .push((expr, scrutinee, pattern, self.started_exprs.len()));
        let has_scrutinee = state.events.contains("scrutinee");
        self.symbolic_match_entries
            .push((scrutinee, pattern, has_scrutinee));
        let state = if has_scrutinee {
            state.with_event("match-entry")
        } else {
            state
        };
        match self.match_event {
            Some(event) => state.with_event(event),
            None => state,
        }
    }

    fn after_expr_control(
        &mut self,
        expr: HirExprId,
        control: crate::intraprocedural::Control<Self::Domain>,
    ) -> crate::intraprocedural::Control<Self::Domain> {
        let control = control.map_states(|state| self.after_expr(expr, state));
        if self.terminal_exprs.contains(&expr) {
            control.with_normal_processed(crate::intraprocedural::Control::error)
        } else {
            control
        }
    }

    fn return_value(
        &mut self,
        stmt: HirStmtId,
        value: Option<HirExprId>,
        state: Self::Domain,
    ) -> Self::Domain {
        self.returned_values.push((stmt, value));
        state
    }

    fn direct_call(
        &mut self,
        call: HirExprId,
        _callee: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if self.incomplete_calls.contains(&call) {
            return AnalysisStep::handled(self.incomplete_facts(state));
        }
        self.handled_calls
            .get(&call)
            .map(|event| AnalysisStep::handled(state.clone().with_event(event)))
            .unwrap_or_else(|| AnalysisStep::unhandled(state, "direct_call"))
    }

    fn perform(
        &mut self,
        _expr: HirExprId,
        _action: &etas_hir::ResolvedActionRef,
        _generic_args: &[etas_hir::HirGenericArg],
        _args: &[etas_hir::HirArg],
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn method_call(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn stage_compose(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn pipeline(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn try_expr(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn handle_expr(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn lambda_boundary(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn handler_boundary(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }
}

#[derive(Default)]
struct HirBuilder {
    program: HirProgram,
}

impl HirBuilder {
    fn literal(&mut self) -> HirExprId {
        self.expr(HirExpr::Literal(HirLiteral::Bool {
            value: true,
            span: span(),
        }))
    }

    fn expr(&mut self, expr: HirExpr) -> HirExprId {
        self.program.exprs.alloc(expr)
    }

    fn stmt(&mut self, stmt: HirStmt) -> HirStmtId {
        self.program.stmts.alloc(stmt)
    }

    fn expr_stmt(&mut self, expr: HirExprId) -> HirStmtId {
        self.stmt(HirStmt::Expr { expr, span: span() })
    }

    fn retry_stmt(&mut self, body: HirBlockId) -> HirStmtId {
        self.stmt(HirStmt::Retry {
            limits: Vec::new(),
            body,
            span: span(),
        })
    }

    fn return_stmt(&mut self, value: Option<HirExprId>) -> HirStmtId {
        self.stmt(HirStmt::Return {
            value,
            span: span(),
        })
    }

    fn block(&mut self, stmts: Vec<HirStmtId>, final_expr: Option<HirExprId>) -> HirBlockId {
        self.program.blocks.alloc_with_id(|id| HirBlock {
            id,
            stmts,
            final_expr,
            scope: ScopeId(0),
            span: span(),
        })
    }

    fn finish_with_roots(mut self, roots: impl IntoIterator<Item = HirBlockId>) -> HirProgram {
        let roots = roots.into_iter().collect::<Vec<_>>();
        let items = roots
            .into_iter()
            .map(|body| {
                self.program.items.alloc(HirItem::Flow(HirFlowDecl {
                    symbol: SymbolId(0),
                    type_params: Vec::new(),
                    params: Vec::new(),
                    return_type: None,
                    effects: None,
                    conformances: Vec::new(),
                    body: HirFlowBody::Block(body),
                    scope: ScopeId(0),
                    span: span(),
                }))
            })
            .collect::<Vec<_>>();
        let module = self.program.modules_arena.alloc_with_id(|id| HirModule {
            id,
            name: None,
            imports: Vec::new(),
            items,
            scope: ScopeId(0),
            span: span(),
        });
        let scope = self
            .program
            .scopes
            .alloc(None, ScopeOwner::Module(module), span());
        self.program
            .modules_arena
            .get_mut(module)
            .expect("new test module should exist")
            .scope = scope;
        self.program.modules.push(module);
        self.program
    }
}

fn span() -> Span {
    Span::empty(SourceId(0), TextSize::ZERO)
}

fn cancellation_checkpoint(
    signal: &std::sync::atomic::AtomicBool,
) -> Result<(), etas_utils::PipelineInterruption> {
    if signal.load(std::sync::atomic::Ordering::Acquire) {
        Err(etas_utils::PipelineInterruption {
            kind: etas_utils::PipelineInterruptionKind::Cancelled,
            reason: "cancel within HIR body".into(),
        })
    } else {
        Ok(())
    }
}

#[test]
fn match_arm_binding_hook_receives_the_scrutinee_state_before_each_branch() {
    let mut builder = HirBuilder::default();
    let scrutinee = builder.literal();
    let first = builder.literal();
    let second = builder.literal();
    let block = builder.block(Vec::new(), Some(second));
    let patterns = [0, 1].map(|_| {
        builder
            .program
            .pats
            .alloc(etas_hir::HirPat::Wildcard { span: span() })
    });
    let matched = builder.expr(HirExpr::Match {
        scrutinee,
        arms: vec![
            etas_hir::HirMatchArm {
                pat: patterns[0],
                body: etas_hir::HirMatchArmBody::Expr(first),
                scope: ScopeId(0),
                span: span(),
            },
            etas_hir::HirMatchArm {
                pat: patterns[1],
                body: etas_hir::HirMatchArmBody::Block(block),
                scope: ScopeId(0),
                span: span(),
            },
        ],
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(matched));
    let semantics =
        TraceSemantics::new(builder.finish_with_roots([root])).event(scrutinee, "scrutinee");
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let output = interpreter.expr(matched, EventDomain::initial());
    assert!(output.events.contains("match-entry"));
    assert_eq!(
        interpreter.semantics().symbolic_match_entries,
        [
            (scrutinee, patterns[0], true),
            (scrutinee, patterns[1], true)
        ]
    );
}

#[test]
fn interruption_inside_one_block_stops_later_statements_and_completion_hooks() {
    let mut builder = HirBuilder::default();
    let expressions = (0..128).map(|_| builder.literal()).collect::<Vec<_>>();
    let statements = expressions
        .iter()
        .map(|expr| builder.expr_stmt(*expr))
        .collect::<Vec<_>>();
    let root = builder.block(statements.clone(), None);
    let signal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    semantics.cancel_after_expr = Some((expressions[0], signal.clone()));
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let error = interpreter
        .try_block_control(root, EventDomain::initial(), &|| {
            cancellation_checkpoint(&signal)
        })
        .unwrap_err();
    assert_eq!(error.kind, etas_utils::PipelineInterruptionKind::Cancelled);
    let semantics = interpreter.semantics();
    assert_eq!(semantics.started_exprs, [expressions[0]]);
    assert_eq!(semantics.completed_exprs, [expressions[0]]);
    assert_eq!(semantics.started_stmts, [statements[0]]);
    assert!(semantics.completed_stmts.is_empty());
}

#[test]
fn interruption_aborts_iterative_structural_and_method_chains() {
    for method_chain in [false, true] {
        let mut builder = HirBuilder::default();
        let leaf = builder.literal();
        let mut current = leaf;
        for _ in 0..256 {
            current = if method_chain {
                builder.expr(HirExpr::MethodCall {
                    receiver: current,
                    method: "step".into(),
                    generic_args: Vec::new(),
                    args: Vec::new(),
                    span: span(),
                })
            } else {
                builder.expr(HirExpr::Unary {
                    op: etas_hir::HirUnaryOp::Not,
                    expr: current,
                    span: span(),
                })
            };
        }
        let root = builder.block(Vec::new(), Some(current));
        let signal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
        semantics.cancel_after_expr = Some((leaf, signal.clone()));
        let mut interpreter = HirAbstractInterpreter::new(semantics);
        assert!(
            interpreter
                .try_expr_control(
                    current,
                    EventDomain::initial(),
                    &|| cancellation_checkpoint(&signal)
                )
                .is_err()
        );
        assert_eq!(interpreter.semantics().completed_exprs, [leaf]);
    }
}

#[test]
fn interruption_in_retry_body_does_not_complete_loop_or_repeat_work() {
    let mut builder = HirBuilder::default();
    let first = builder.literal();
    let later = builder.literal();
    let first_stmt = builder.expr_stmt(first);
    let later_stmt = builder.expr_stmt(later);
    let body = builder.block(vec![first_stmt, later_stmt], None);
    let retry = builder.retry_stmt(body);
    let root = builder.block(vec![retry], None);
    let signal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    semantics.cancel_after_expr = Some((first, signal.clone()));
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    assert!(
        interpreter
            .try_stmt_control(retry, EventDomain::initial(), &|| cancellation_checkpoint(
                &signal
            ))
            .is_err()
    );
    let semantics = interpreter.semantics();
    assert_eq!(semantics.started_stmts, [retry, first_stmt]);
    assert_eq!(semantics.completed_exprs, [first]);
    assert!(semantics.completed_stmts.is_empty());
}

#[test]
fn fallible_and_infallible_traversals_preserve_the_same_control_and_hook_order() {
    let mut builder = HirBuilder::default();
    let cond = builder.literal();
    let returned = builder.literal();
    let ret_stmt = builder.return_stmt(Some(returned));
    let then_block = builder.block(vec![ret_stmt], None);
    let else_value = builder.literal();
    let else_block = builder.block(Vec::new(), Some(else_value));
    let expr = builder.expr(HirExpr::If {
        cond,
        then_block,
        else_branch: Some(HirElseBranch::Block(else_block)),
        span: span(),
    });
    let stmt = builder.stmt(HirStmt::If(expr));
    let root = builder.block(vec![stmt], None);
    let hir = builder.finish_with_roots([root]);
    let mut ordinary = HirAbstractInterpreter::new(TraceSemantics::new(hir.clone()));
    let mut controlled = HirAbstractInterpreter::new(TraceSemantics::new(hir));
    let expected = ordinary.block_control(root, EventDomain::initial());
    let actual = controlled
        .try_block_control(root, EventDomain::initial(), &|| Ok::<(), ()>(()))
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        controlled.semantics().started_exprs,
        ordinary.semantics().started_exprs
    );
    assert_eq!(
        controlled.semantics().completed_exprs,
        ordinary.semantics().completed_exprs
    );
    assert_eq!(
        controlled.semantics().started_stmts,
        ordinary.semantics().started_stmts
    );
    assert_eq!(
        controlled.semantics().completed_stmts,
        ordinary.semantics().completed_stmts
    );
    assert_eq!(
        controlled.semantics().returned_values,
        ordinary.semantics().returned_values
    );
}

#[test]
fn visits_block_statements_and_final_expr_in_ordered_structure() {
    let mut builder = HirBuilder::default();
    let first = builder.literal();
    let second = builder.literal();
    let stmt = builder.expr_stmt(first);
    let block = builder.block(vec![stmt], Some(second));
    let semantics = TraceSemantics::new(builder.finish_with_roots([block]))
        .event(first, "stmt-expr")
        .event(second, "final-expr");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(block, EventDomain::initial());

    assert!(result.events.contains("stmt-expr"));
    assert!(result.events.contains("final-expr"));
    assert!(!result.incomplete);
}

#[test]
fn joins_if_branches() {
    let mut builder = HirBuilder::default();
    let cond = builder.literal();
    let then_expr = builder.literal();
    let else_expr = builder.literal();
    let then_block = builder.block(Vec::new(), Some(then_expr));
    let else_block = builder.block(Vec::new(), Some(else_expr));
    let if_expr = builder.expr(HirExpr::If {
        cond,
        then_block,
        else_branch: Some(HirElseBranch::Block(else_block)),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(if_expr));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]))
        .event(cond, "cond")
        .event(then_expr, "then")
        .event(else_expr, "else");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    assert!(result.events.contains("cond"));
    assert!(result.events.contains("then"));
    assert!(result.events.contains("else"));
}

#[test]
fn match_pattern_hooks_run_after_scrutinee_and_before_each_arm() {
    for terminal in [false, true] {
        let mut builder = HirBuilder::default();
        let scrutinee = builder.literal();
        let first = builder.literal();
        let second = builder.literal();
        let patterns = [0, 1].map(|_| {
            builder
                .program
                .pats
                .alloc(etas_hir::HirPat::Wildcard { span: span() })
        });
        let expression = builder.expr(HirExpr::Match {
            scrutinee,
            arms: vec![
                etas_hir::HirMatchArm {
                    pat: patterns[0],
                    body: etas_hir::HirMatchArmBody::Expr(first),
                    scope: ScopeId(0),
                    span: span(),
                },
                etas_hir::HirMatchArm {
                    pat: patterns[1],
                    body: etas_hir::HirMatchArmBody::Expr(second),
                    scope: ScopeId(0),
                    span: span(),
                },
            ],
            span: span(),
        });
        let root = builder.block(vec![], Some(expression));
        let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
        semantics.match_event = Some("bound");
        if terminal {
            semantics.terminal_exprs.insert(scrutinee);
        }
        let mut interpreter = HirAbstractInterpreter::new(semantics);
        let control = interpreter.expr_control(expression, EventDomain::initial());
        let semantics = interpreter.semantics();
        if terminal {
            assert!(semantics.match_entries.is_empty());
            assert!(!semantics.started_exprs.contains(&first));
            assert!(control.normal_state().is_none());
        } else {
            assert_eq!(
                semantics.match_entries,
                [
                    (expression, scrutinee, patterns[0], 2),
                    (expression, scrutinee, patterns[1], 3)
                ]
            );
            assert!(control.normal_state().unwrap().events.contains("bound"));
            assert_eq!(
                semantics.started_exprs,
                [expression, scrutinee, first, second]
            );
        }
    }
}

#[test]
fn visits_else_if_chain_without_recursive_control_walk() {
    let mut builder = HirBuilder::default();
    let outer_cond = builder.literal();
    let outer_then = builder.literal();
    let inner_cond = builder.literal();
    let inner_then = builder.literal();
    let inner_else = builder.literal();
    let outer_then_block = builder.block(Vec::new(), Some(outer_then));
    let inner_then_block = builder.block(Vec::new(), Some(inner_then));
    let inner_else_block = builder.block(Vec::new(), Some(inner_else));
    let inner_if = builder.expr(HirExpr::If {
        cond: inner_cond,
        then_block: inner_then_block,
        else_branch: Some(HirElseBranch::Block(inner_else_block)),
        span: span(),
    });
    let outer_if = builder.expr(HirExpr::If {
        cond: outer_cond,
        then_block: outer_then_block,
        else_branch: Some(HirElseBranch::If(inner_if)),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(outer_if));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]))
        .event(outer_cond, "outer-cond")
        .event(outer_then, "outer-then")
        .event(inner_cond, "inner-cond")
        .event(inner_then, "inner-then")
        .event(inner_else, "inner-else");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    for event in [
        "outer-cond",
        "outer-then",
        "inner-cond",
        "inner-then",
        "inner-else",
    ] {
        assert!(result.events.contains(event), "missing event {event}");
    }
    assert!(!result.incomplete);
}

#[test]
fn visits_statement_position_if_inside_else_block_without_losing_events() {
    let mut builder = HirBuilder::default();
    let outer_cond = builder.literal();
    let outer_then = builder.literal();
    let inner_cond = builder.literal();
    let inner_then = builder.literal();
    let outer_then_block = builder.block(Vec::new(), Some(outer_then));
    let inner_then_block = builder.block(Vec::new(), Some(inner_then));
    let inner_if = builder.expr(HirExpr::If {
        cond: inner_cond,
        then_block: inner_then_block,
        else_branch: None,
        span: span(),
    });
    let inner_stmt = builder.stmt(HirStmt::If(inner_if));
    let else_block = builder.block(vec![inner_stmt], None);
    let outer_if = builder.expr(HirExpr::If {
        cond: outer_cond,
        then_block: outer_then_block,
        else_branch: Some(HirElseBranch::Block(else_block)),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(outer_if));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]))
        .event(outer_cond, "outer-cond")
        .event(outer_then, "outer-then")
        .event(inner_cond, "inner-cond")
        .event(inner_then, "inner-then")
        .before_stmt_event(inner_stmt, "inner-stmt-before")
        .after_stmt_event(inner_stmt, "inner-stmt-after");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    for event in [
        "outer-cond",
        "outer-then",
        "inner-cond",
        "inner-then",
        "inner-stmt-before",
        "inner-stmt-after",
    ] {
        assert!(result.events.contains(event), "missing event {event}");
    }
    assert!(!result.incomplete);
}

#[test]
fn solves_loop_body_to_fixpoint() {
    let mut builder = HirBuilder::default();
    let tick = builder.literal();
    let loop_body = builder.block(Vec::new(), Some(tick));
    let retry = builder.retry_stmt(loop_body);
    let root = builder.block(vec![retry], None);
    let semantics = TraceSemantics::new(builder.finish_with_roots([root])).event(tick, "loop-body");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    assert!(result.events.contains("loop-body"));
    assert!(!result.incomplete);
}

#[test]
fn loop_iteration_limit_marks_nonconverged_facts_incomplete() {
    let mut builder = HirBuilder::default();
    let tick = builder.literal();
    let condition = builder.literal();
    let returned = builder.return_stmt(None);
    let then_block = builder.block(vec![returned], None);
    let else_block = builder.block(Vec::new(), None);
    let branch = builder.expr(HirExpr::If {
        cond: condition,
        then_block,
        else_branch: Some(HirElseBranch::Block(else_block)),
        span: span(),
    });
    let branch = builder.stmt(HirStmt::If(branch));
    let tick = builder.expr_stmt(tick);
    let loop_body = builder.block(vec![tick, branch], None);
    let retry = builder.retry_stmt(loop_body);
    let root = builder.block(vec![retry], None);
    let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    semantics.growing_expr = Some(condition);

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let control = interpreter.block_control(root, EventDomain::initial());

    let normal = control.normal_state().expect("normal loop exit");
    let returned = control.return_state().expect("return from loop");
    assert!(normal.growth >= etas_utils::IterationLimit::DEFAULT.max_iterations());
    assert!(normal.incomplete, "normal facts must be incomplete");
    assert!(returned.incomplete, "return facts must be incomplete");
    assert_eq!(interpreter.semantics().incomplete_loops, vec![loop_body]);
}

#[test]
fn delegates_direct_call_without_inlining_source_bodies() {
    let mut builder = HirBuilder::default();
    let callee = builder.literal();
    let source_body_expr = builder.literal();
    let _source_body = builder.block(Vec::new(), Some(source_body_expr));
    let call = builder.expr(HirExpr::Call {
        callee,
        generic_args: Vec::new(),
        args: Vec::new(),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(call));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root, _source_body]))
        .event(source_body_expr, "source-body")
        .handled_call(call, "direct-call");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    assert!(result.events.contains("direct-call"));
    assert!(!result.events.contains("source-body"));
    assert!(!result.incomplete);
}

#[test]
fn marks_missing_call_facts_as_incomplete() {
    let mut builder = HirBuilder::default();
    let callee = builder.literal();
    let call = builder.expr(HirExpr::Call {
        callee,
        generic_args: Vec::new(),
        args: Vec::new(),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(call));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root])).incomplete_call(call);

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    assert!(result.incomplete);
}

#[test]
fn return_control_stops_later_block_statements() {
    let mut builder = HirBuilder::default();
    let returned = builder.literal();
    let after_return = builder.literal();
    let return_stmt = builder.return_stmt(Some(returned));
    let after_stmt = builder.expr_stmt(after_return);
    let root = builder.block(vec![return_stmt, after_stmt], None);
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]))
        .event(returned, "returned")
        .event(after_return, "after-return");

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    assert!(result.events.contains("returned"));
    assert!(!result.events.contains("after-return"));
}

#[test]
fn unhandled_critical_hook_marks_state_incomplete() {
    let mut builder = HirBuilder::default();
    let callee = builder.literal();
    let call = builder.expr(HirExpr::Call {
        callee,
        generic_args: Vec::new(),
        args: Vec::new(),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(call));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]));

    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block(root, EventDomain::initial());

    assert!(result.incomplete);
    assert!(!result.events.contains("direct-call"));
}

#[test]
fn nested_return_is_not_relabeled_as_the_outer_return_value() {
    let mut builder = HirBuilder::default();
    let value = builder.literal();
    let inner_return = builder.return_stmt(Some(value));
    let inner_body = builder.block(vec![inner_return], None);
    let block_expr = builder.expr(HirExpr::Block(inner_body));
    let outer_return = builder.return_stmt(Some(block_expr));
    let root = builder.block(vec![outer_return], None);
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block_control(root, EventDomain::initial());
    assert!(result.return_state().is_some());
    assert!(result.normal_state().is_none());
    assert_eq!(
        interpreter.semantics().returned_values,
        [(inner_return, Some(value))]
    );
}

#[test]
fn expression_control_hook_can_terminate_a_path_before_later_statements() {
    let mut builder = HirBuilder::default();
    let terminal = builder.literal();
    let fallback = builder.literal();
    let terminal_stmt = builder.expr_stmt(terminal);
    let fallback_stmt = builder.expr_stmt(fallback);
    let root = builder.block(vec![terminal_stmt, fallback_stmt], None);
    let mut semantics =
        TraceSemantics::new(builder.finish_with_roots([root])).event(fallback, "fallback");
    semantics.terminal_exprs.insert(terminal);
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let result = interpreter.block_control(root, EventDomain::initial());
    assert!(result.normal_state().is_none());
    assert!(result.error_state().is_some());
    assert_eq!(interpreter.semantics().completed_exprs, [terminal]);
    assert!(!result.into_joined_domain().events.contains("fallback"));
}

#[test]
fn iterative_method_chain_preserves_each_expression_hook_in_nested_order() {
    let mut builder = HirBuilder::default();
    let base = builder.literal();
    let first_arg = builder.literal();
    let second_arg = builder.literal();
    let first = builder.expr(HirExpr::MethodCall {
        receiver: base,
        method: "first".into(),
        generic_args: Vec::new(),
        args: vec![etas_hir::HirArg::Positional(first_arg)],
        span: span(),
    });
    let second = builder.expr(HirExpr::MethodCall {
        receiver: first,
        method: "second".into(),
        generic_args: Vec::new(),
        args: vec![etas_hir::HirArg::Positional(second_arg)],
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(second));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    interpreter.block_control(root, EventDomain::initial());
    assert_eq!(
        interpreter.semantics().started_exprs,
        [second, first, base, first_arg, second_arg]
    );
    assert_eq!(
        interpreter.semantics().completed_exprs,
        [base, first_arg, first, second_arg, second]
    );
}

#[test]
fn iterative_else_if_preserves_inner_expression_hooks() {
    let mut builder = HirBuilder::default();
    let cond = builder.literal();
    let nested_cond = builder.literal();
    let then_value = builder.literal();
    let nested_then = builder.literal();
    let nested_else = builder.literal();
    let then_block = builder.block(Vec::new(), Some(then_value));
    let inner_then_block = builder.block(Vec::new(), Some(nested_then));
    let inner_else_block = builder.block(Vec::new(), Some(nested_else));
    let inner = builder.expr(HirExpr::If {
        cond: nested_cond,
        then_block: inner_then_block,
        else_branch: Some(HirElseBranch::Block(inner_else_block)),
        span: span(),
    });
    let outer = builder.expr(HirExpr::If {
        cond,
        then_block,
        else_branch: Some(HirElseBranch::If(inner)),
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(outer));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    interpreter.block_control(root, EventDomain::initial());
    assert_eq!(
        interpreter.semantics().started_exprs,
        [
            outer,
            cond,
            then_value,
            inner,
            nested_cond,
            nested_then,
            nested_else
        ]
    );
    assert_eq!(
        interpreter.semantics().completed_exprs,
        [
            cond,
            then_value,
            nested_cond,
            nested_then,
            nested_else,
            inner,
            outer
        ]
    );
}

#[test]
fn deep_structural_expressions_use_a_worklist_and_preserve_all_hooks() {
    let mut builder = HirBuilder::default();
    let leaf = builder.literal();
    let mut expression = leaf;
    let mut started = vec![leaf];
    for _ in 0..10_000 {
        expression = builder.expr(HirExpr::Unary {
            op: etas_hir::HirUnaryOp::Not,
            expr: expression,
            span: span(),
        });
        started.push(expression);
    }
    let root = builder.block(Vec::new(), Some(expression));
    let semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let control = interpreter.block_control(root, EventDomain::initial());
    assert!(control.normal_state().is_some());
    assert_eq!(interpreter.semantics().completed_exprs, started);
    started.reverse();
    assert_eq!(interpreter.semantics().started_exprs, started);
}

#[test]
fn structural_worklist_skips_later_operands_after_a_terminal_child() {
    let mut builder = HirBuilder::default();
    let terminal = builder.literal();
    let later = builder.literal();
    let inner = builder.expr(HirExpr::Unary {
        op: etas_hir::HirUnaryOp::Not,
        expr: terminal,
        span: span(),
    });
    let outer = builder.expr(HirExpr::Binary {
        op: etas_hir::HirBinaryOp::EqEq,
        lhs: inner,
        rhs: later,
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(outer));
    let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    semantics.terminal_exprs.insert(terminal);
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let control = interpreter.block_control(root, EventDomain::initial());
    assert!(control.normal_state().is_none());
    assert!(control.error_state().is_some());
    assert_eq!(
        interpreter.semantics().started_exprs,
        [outer, inner, terminal]
    );
    assert_eq!(
        interpreter.semantics().completed_exprs,
        [terminal, inner, outer]
    );
}

#[test]
fn a_later_operand_hook_does_not_touch_an_earlier_operands_return_channel() {
    let mut builder = HirBuilder::default();
    let condition = builder.literal();
    let returned = builder.literal();
    let returned_stmt = builder.return_stmt(Some(returned));
    let then_block = builder.block(vec![returned_stmt], None);
    let else_value = builder.literal();
    let else_block = builder.block(Vec::new(), Some(else_value));
    let lhs = builder.expr(HirExpr::If {
        cond: condition,
        then_block,
        else_branch: Some(HirElseBranch::Block(else_block)),
        span: span(),
    });
    let rhs_value = builder.literal();
    let rhs = builder.expr(HirExpr::Unary {
        op: etas_hir::HirUnaryOp::Not,
        expr: rhs_value,
        span: span(),
    });
    let expression = builder.expr(HirExpr::Binary {
        op: etas_hir::HirBinaryOp::EqEq,
        lhs,
        rhs,
        span: span(),
    });
    let root = builder.block(Vec::new(), Some(expression));
    let mut semantics = TraceSemantics::new(builder.finish_with_roots([root]));
    semantics.after_expr_events.insert(rhs, "rhs completed");
    let mut interpreter = HirAbstractInterpreter::new(semantics);
    let control = interpreter.block_control(root, EventDomain::initial());
    assert!(
        control
            .normal_state()
            .unwrap()
            .events
            .contains("rhs completed")
    );
    assert!(
        !control
            .return_state()
            .unwrap()
            .events
            .contains("rhs completed")
    );
}
