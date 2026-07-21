use std::collections::{BTreeMap, BTreeSet};

use etas_core::{SourceId, Span, TextSize};
use etas_hir::{
    HirBlock, HirBlockId, HirExpr, HirExprId, HirFlowBody, HirFlowDecl, HirItem, HirLiteral,
    HirModule, HirProgram, HirStmt, HirStmtId, ScopeId, ScopeOwner, SymbolId,
};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::{
    interprocedural::{
        CallSite, CallTarget, HirAnalysisBody, InterproceduralAnalysis, InterproceduralSemantics,
        UnitContext,
    },
    intraprocedural::{AnalysisStep, Control, HirAnalysisSemantics},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Unit(u32);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EventDomain {
    events: BTreeSet<&'static str>,
    incomplete: bool,
}

impl EventDomain {
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
struct TraceInterproceduralSemantics {
    hir: HirProgram,
    bodies: BTreeMap<Unit, HirAnalysisBody>,
    expr_events: BTreeMap<HirExprId, &'static str>,
    call_targets: BTreeMap<HirExprId, CallTarget<Unit>>,
}

impl TraceInterproceduralSemantics {
    fn new(hir: HirProgram) -> Self {
        Self {
            hir,
            ..Self::default()
        }
    }

    fn body(mut self, unit: Unit, body: HirBlockId) -> Self {
        self.bodies.insert(unit, HirAnalysisBody::Block(body));
        self
    }

    fn expr_body(mut self, unit: Unit, body: HirExprId) -> Self {
        self.bodies.insert(unit, HirAnalysisBody::Expr(body));
        self
    }

    fn external_body(mut self, unit: Unit) -> Self {
        self.bodies.insert(unit, HirAnalysisBody::External);
        self
    }

    fn missing_body(mut self, unit: Unit) -> Self {
        self.bodies.insert(unit, HirAnalysisBody::Missing);
        self
    }

    fn event(mut self, expr: HirExprId, event: &'static str) -> Self {
        self.expr_events.insert(expr, event);
        self
    }

    fn call_target(mut self, call: HirExprId, target: CallTarget<Unit>) -> Self {
        self.call_targets.insert(call, target);
        self
    }
}

impl HirAnalysisSemantics for TraceInterproceduralSemantics {
    type Domain = EventDomain;

    fn hir(&self) -> &HirProgram {
        &self.hir
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        state.with_incomplete()
    }

    fn before_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        self.expr_events
            .get(&expr)
            .map(|event| state.clone().with_event(event))
            .unwrap_or(state)
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

    fn direct_call(
        &mut self,
        _call: HirExprId,
        _callee: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::unhandled(state, "direct_call")
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

impl InterproceduralSemantics for TraceInterproceduralSemantics {
    type Unit = Unit;
    type Domain = EventDomain;
    type Summary = EventDomain;

    fn body_of(&self, unit: Self::Unit) -> HirAnalysisBody {
        self.bodies
            .get(&unit)
            .copied()
            .unwrap_or(HirAnalysisBody::Missing)
    }

    fn begin_unit(&mut self, _context: UnitContext<Self::Unit>) -> Self::Domain {
        EventDomain::bottom()
    }

    fn end_unit(
        &mut self,
        _context: UnitContext<Self::Unit>,
        exit: Control<Self::Domain>,
    ) -> Self::Summary {
        exit.into_joined_domain()
    }

    fn external_summary(&mut self, _context: UnitContext<Self::Unit>) -> Self::Summary {
        EventDomain::bottom().with_event("external-summary")
    }

    fn missing_summary(&mut self, _context: UnitContext<Self::Unit>) -> Self::Summary {
        EventDomain::bottom().with_incomplete()
    }

    fn call_target(
        &mut self,
        _context: UnitContext<Self::Unit>,
        call: HirExprId,
        _callee: HirExprId,
        _state: &Self::Domain,
    ) -> CallTarget<Self::Unit> {
        self.call_targets
            .get(&call)
            .copied()
            .unwrap_or(CallTarget::External)
    }

    fn direct_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        _site: CallSite<Self::Unit>,
        _callee: Self::Unit,
        summary: &Self::Summary,
        mut state: Self::Domain,
    ) -> Self::Domain {
        state.join_assign(summary);
        state
    }

    fn dynamic_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        _site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        state.with_event("dynamic-call")
    }

    fn external_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        _site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        state.with_event("external-call")
    }

    fn incomplete_call(
        &mut self,
        _context: UnitContext<Self::Unit>,
        _site: CallSite<Self::Unit>,
        state: Self::Domain,
    ) -> Self::Domain {
        state.with_incomplete()
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

    fn call(&mut self) -> HirExprId {
        let callee = self.literal();
        self.expr(HirExpr::Call {
            callee,
            generic_args: Vec::new(),
            args: Vec::new(),
            span: span(),
        })
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
fn solves_known_call_by_applying_callee_summary() {
    let mut builder = HirBuilder::default();
    let callee_event = builder.literal();
    let callee_body = builder.block(Vec::new(), Some(callee_event));
    let call = builder.call();
    let caller_body = builder.block(Vec::new(), Some(call));
    let semantics =
        TraceInterproceduralSemantics::new(builder.finish_with_roots([caller_body, callee_body]))
            .body(Unit(0), caller_body)
            .body(Unit(1), callee_body)
            .event(callee_event, "callee-body")
            .call_target(call, CallTarget::Direct(Unit(1)));

    let result = InterproceduralAnalysis::new([Unit(0), Unit(1)], semantics).solve();

    assert!(result.diagnostics.is_empty());
    assert_eq!(result.call_graph.dependencies(Unit(0)), vec![Unit(1)]);
    let caller = result.summaries.get(Unit(0)).expect("caller summary");
    assert!(caller.events.contains("callee-body"));
    assert!(!caller.incomplete);
}

#[test]
fn solves_mutually_recursive_units_by_scc_fixpoint() {
    let mut builder = HirBuilder::default();
    let a_event = builder.literal();
    let a_call_b = builder.call();
    let a_stmt = builder.expr_stmt(a_event);
    let a_body = builder.block(vec![a_stmt], Some(a_call_b));
    let b_event = builder.literal();
    let b_call_a = builder.call();
    let b_stmt = builder.expr_stmt(b_event);
    let b_body = builder.block(vec![b_stmt], Some(b_call_a));
    let semantics = TraceInterproceduralSemantics::new(builder.finish_with_roots([a_body, b_body]))
        .body(Unit(0), a_body)
        .body(Unit(1), b_body)
        .event(a_event, "a")
        .event(b_event, "b")
        .call_target(a_call_b, CallTarget::Direct(Unit(1)))
        .call_target(b_call_a, CallTarget::Direct(Unit(0)));

    let result = InterproceduralAnalysis::new([Unit(0), Unit(1)], semantics).solve();

    assert!(result.diagnostics.is_empty());
    let a = result.summaries.get(Unit(0)).expect("a summary");
    let b = result.summaries.get(Unit(1)).expect("b summary");
    assert!(a.events.contains("a"));
    assert!(a.events.contains("b"));
    assert!(b.events.contains("a"));
    assert!(b.events.contains("b"));
    assert_eq!(result.convergence.len(), 1);
}

#[test]
fn dispatches_dynamic_external_and_incomplete_calls_separately() {
    let mut builder = HirBuilder::default();
    let dynamic_call = builder.call();
    let external_call = builder.call();
    let incomplete_call = builder.call();
    let dynamic_stmt = builder.expr_stmt(dynamic_call);
    let external_stmt = builder.expr_stmt(external_call);
    let incomplete_stmt = builder.expr_stmt(incomplete_call);
    let body = builder.block(vec![dynamic_stmt, external_stmt, incomplete_stmt], None);
    let semantics = TraceInterproceduralSemantics::new(builder.finish_with_roots([body]))
        .body(Unit(0), body)
        .call_target(dynamic_call, CallTarget::Dynamic)
        .call_target(external_call, CallTarget::External)
        .call_target(incomplete_call, CallTarget::Incomplete);

    let result = InterproceduralAnalysis::new([Unit(0)], semantics).solve();

    assert!(result.diagnostics.is_empty());
    assert!(result.call_graph.dependencies(Unit(0)).is_empty());
    let summary = result.summaries.get(Unit(0)).expect("summary");
    assert!(summary.events.contains("dynamic-call"));
    assert!(summary.events.contains("external-call"));
    assert!(summary.incomplete);
}

#[test]
fn solves_expr_external_and_missing_bodies_without_fabricated_blocks() {
    let mut builder = HirBuilder::default();
    let expr = builder.literal();
    let owner = builder.block(Vec::new(), Some(expr));
    let semantics = TraceInterproceduralSemantics::new(builder.finish_with_roots([owner]))
        .expr_body(Unit(0), expr)
        .external_body(Unit(1))
        .missing_body(Unit(2))
        .event(expr, "expr-body");

    let result = InterproceduralAnalysis::new([Unit(0), Unit(1), Unit(2)], semantics).solve();

    let expr_summary = result.summaries.get(Unit(0)).expect("expr summary");
    let external_summary = result.summaries.get(Unit(1)).expect("external summary");
    let missing_summary = result.summaries.get(Unit(2)).expect("missing summary");
    assert!(expr_summary.events.contains("expr-body"));
    assert!(external_summary.events.contains("external-summary"));
    assert!(missing_summary.incomplete);
    assert_eq!(result.diagnostics.len(), 1);
}
