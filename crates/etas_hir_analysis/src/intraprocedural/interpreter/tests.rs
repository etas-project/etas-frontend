use std::collections::{BTreeMap, BTreeSet};

use etas_core::{SourceId, Span, TextSize};
use etas_hir::{
    HirBlock, HirBlockId, HirElseBranch, HirExpr, HirExprId, HirFlowBody, HirFlowDecl, HirItem,
    HirLiteral, HirModule, HirProgram, HirStmt, HirStmtId, ScopeId, ScopeOwner, SymbolId,
};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::intraprocedural::{AnalysisStep, HirAbstractInterpreter, HirAnalysisSemantics};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EventDomain {
    events: BTreeSet<&'static str>,
    incomplete: bool,
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
        self.events.is_subset(&other.events) && (!self.incomplete || other.incomplete)
    }
}

impl JoinSemiLattice for EventDomain {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        let old_len = self.events.len();
        let old_incomplete = self.incomplete;
        self.events.extend(other.events.iter().copied());
        self.incomplete |= other.incomplete;
        self.events.len() != old_len || self.incomplete != old_incomplete
    }
}

#[derive(Default)]
struct TraceSemantics {
    hir: HirProgram,
    expr_events: BTreeMap<HirExprId, &'static str>,
    before_stmt_events: BTreeMap<HirStmtId, &'static str>,
    after_stmt_events: BTreeMap<HirStmtId, &'static str>,
    incomplete_calls: BTreeSet<HirExprId>,
    handled_calls: BTreeMap<HirExprId, &'static str>,
}

impl TraceSemantics {
    fn new(hir: HirProgram) -> Self {
        Self {
            hir,
            ..Self::default()
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

    fn hir(&self) -> &HirProgram {
        &self.hir
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        state.with_incomplete()
    }

    fn before_stmt(&mut self, stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        self.before_stmt_events
            .get(&stmt)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
    }

    fn after_stmt(&mut self, stmt: HirStmtId, state: Self::Domain) -> Self::Domain {
        self.after_stmt_events
            .get(&stmt)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
    }

    fn before_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        self.expr_events
            .get(&expr)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
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
