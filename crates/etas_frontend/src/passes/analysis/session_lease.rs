use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use etas_core::{Diagnostic, Span, TypeDiagnosticCode};
use etas_hir::{
    HirArg, HirBlockId, HirExpr, HirExprId, HirFieldInit, HirGenericArg, HirLiteral, HirOwner,
    HirProgram, HirRevision, HirStmt, HirStmtId, PartialResolutionReason, ResolveResult,
    ResolvedActionRef, SymbolDef, SymbolId,
};
use etas_hir_analysis::HirAnalysisContext;
use etas_hir_analysis::alias::{AliasTarget, Place, Projection};
use etas_hir_analysis::intraprocedural::{
    AnalysisStep, Control, HirAbstractInterpreter, HirAnalysisSemantics,
};
use etas_types::{SymbolTypeFact, Type, TypeId, TypeOutput};

mod captures;
mod generic;
use super::lexical_capture::CaptureLayouts;

const LEASE: &str = "std.agent.session.SessionLease";
const WITH_LEASE: [&str; 4] = ["std", "agent", "session", "with_lease"];

pub(super) fn check(
    hir: &HirRevision,
    types: &mut TypeOutput,
    layouts: &CaptureLayouts,
) -> Result<Vec<Diagnostic>, String> {
    let applications = generic::applications(hir, types)?;
    let mut diagnostics = generic::infer_copy_requirements(hir, types, layouts, &applications);
    let Some(lease) = types.facts.known_std_types.opaque_values.iter().copied().find(
        |ty| matches!(types.store.get(*ty), Some(Type::Nominal(nominal)) if nominal.name == LEASE),
    ) else {
        return Ok(diagnostics);
    };

    let mut analysis = LeaseAnalysis::new(HirAnalysisContext::new(hir), types, lease, layouts);
    analysis.propagate_generic_bindings(&applications);
    let mut owners = HashSet::new();
    for (expr, ty) in &types.facts.expr_types {
        if !analysis.contains_lease(*ty) {
            continue;
        }
        if let Some(item) = hir.index().expr_item.get(expr) {
            owners.insert(*item);
        }
        if is_with_lease(hir, types, *expr) {
            diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                hir.exprs[*expr].span(&hir.blocks),
                "SessionLease cannot escape the scope created by with_lease",
            ));
        }
    }
    let affine_symbols = types
        .facts
        .symbol_types
        .iter()
        .filter_map(|(symbol, fact)| {
            let ty = value_type(fact)?;
            analysis.contains_lease(ty).then_some(*symbol)
        })
        .collect::<HashSet<_>>();
    analysis.captures.index(hir, types, affine_symbols);
    owners.extend(analysis.captures.callbacks.keys().map(|body| body.item));
    for (expr, node) in hir.exprs.iter() {
        if let HirExpr::Record(record) = node
            && record.fields.iter().any(|field| {
                matches!(
                    field,
                    HirFieldInit::Shorthand { resolution: ResolveResult::Resolved(symbol), .. }
                        if analysis.captures.affine_symbols.contains(symbol)
                )
            })
            && let Some(owner) = hir.index().expr_item.get(&expr)
        {
            owners.insert(*owner);
        }
    }
    let mut interpreter = HirAbstractInterpreter::new(analysis);
    for (item, _) in hir.items.iter().filter(|(item, _)| owners.contains(item)) {
        analyze_owner(hir, item, &mut interpreter);
    }
    let analysis = interpreter.into_semantics();
    diagnostics.extend(analysis.captures.escapes.into_iter().map(|span| Diagnostic::type_check(
        TypeDiagnosticCode::TypeMismatch, span, "SessionLease cannot escape its owning scope through the returned value or stored state",
    )));
    diagnostics.extend(analysis.moved_uses.into_iter().map(|span| {
        Diagnostic::type_check(
            TypeDiagnosticCode::TypeMismatch,
            span,
            "use of a moved SessionLease",
        )
    }));
    diagnostics.sort_by_key(|diagnostic| {
        let span = diagnostic.primary.span;
        (span.source.0, span.range.start, span.range.end)
    });
    Ok(diagnostics)
}

fn analyze_owner(
    hir: &HirRevision,
    item: etas_hir::HirItemId,
    interpreter: &mut HirAbstractInterpreter<LeaseAnalysis<'_>>,
) {
    for body in hir
        .index()
        .item_bodies
        .get(&item)
        .into_iter()
        .flatten()
        .copied()
    {
        // Match arms are already traversed with the scrutinee's state.
        if matches!(body.kind, etas_hir::HirBodyKind::MatchArm { .. }) {
            continue;
        }
        interpreter.semantics_mut().captures.enter_body(hir, body);
        if let Some(block) = interpreter.context().body_block(body) {
            interpreter.block_control(block, LeaseState::default());
        } else if let Some(expr) = interpreter.context().body_root_expr(body) {
            interpreter.expr_control(expr, LeaseState::default());
        }
    }
}

fn is_with_lease(hir: &HirProgram, types: &TypeOutput, expr: HirExprId) -> bool {
    if types
        .facts
        .library_flow_calls
        .get(&expr)
        .and_then(|call| call.callable.as_declaration())
        .is_some_and(|id| id.module() == &WITH_LEASE[..3] && id.declaration() == &WITH_LEASE[3..])
    {
        return true;
    }
    let HirExpr::Call { callee, .. } = &hir.exprs[expr] else {
        return false;
    };
    etas_types::source_execution::callable_symbol(hir, *callee)
        .and_then(|symbol| hir.declaration_path(symbol))
        .is_some_and(|path| path == WITH_LEASE)
}

#[derive(Clone, Default)]
struct LeaseState {
    moved: BTreeSet<Place>,
    // Include empty entries so an alias keeps the value it received even if
    // the mutable binding it came from is reassigned later.
    captured: BTreeMap<SymbolId, BTreeSet<SymbolId>>,
}

impl etas_utils::PartialOrder for LeaseState {
    fn less_equal(&self, other: &Self) -> bool {
        self.moved.is_subset(&other.moved)
            && self.captured.iter().all(|(symbol, origins)| {
                other
                    .captured
                    .get(symbol)
                    .is_some_and(|set| origins.is_subset(set))
            })
    }
}

impl etas_utils::JoinSemiLattice for LeaseState {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        if etas_utils::PartialOrder::less_equal(other, self) {
            return false;
        }
        self.moved.extend(other.moved.iter().cloned());
        for (symbol, origins) in &other.captured {
            self.captured.entry(*symbol).or_default().extend(origins);
        }
        true
    }
}

struct LeaseAnalysis<'a> {
    context: HirAnalysisContext,
    types: &'a TypeOutput,
    lease: TypeId,
    contains: HashMap<TypeId, bool>,
    moved_uses: HashSet<Span>,
    captures: captures::LeaseCaptures<'a>,
}

impl<'a> LeaseAnalysis<'a> {
    fn new(
        context: HirAnalysisContext,
        types: &'a TypeOutput,
        lease: TypeId,
        layouts: &'a CaptureLayouts,
    ) -> Self {
        Self {
            context,
            types,
            lease,
            contains: HashMap::new(),
            moved_uses: HashSet::new(),
            captures: captures::LeaseCaptures::new(layouts),
        }
    }

    /// Type parameter subjects belong to their declaring callable. A wrapper
    /// may pass its own parameter on to another generic flow or method.
    fn propagate_generic_bindings(&mut self, applications: &[generic::Application]) {
        let edges: Vec<_> = applications
            .iter()
            .filter_map(|use_| use_.local_binding())
            .collect();
        loop {
            let new: HashSet<_> = edges
                .iter()
                .filter_map(|(param, actual)| {
                    (self.contains.get(param) != Some(&true) && self.contains_lease(*actual))
                        .then_some(*param)
                })
                .collect();
            if new.is_empty() {
                break;
            }
            self.contains.retain(|_, contains| *contains);
            self.contains
                .extend(new.into_iter().map(|param| (param, true)));
        }
    }

    fn contains_lease(&mut self, root: TypeId) -> bool {
        if let Some(result) = self.contains.get(&root) {
            return *result;
        }
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        let mut result = false;
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) || self.contains.get(&ty) == Some(&false) {
                continue;
            }
            if ty == self.lease || self.contains.get(&ty) == Some(&true) {
                result = true;
                break;
            }
            match self.types.store.get(ty) {
                Some(Type::Record(record)) => pending.extend(record.fields.iter().map(|f| f.ty)),
                Some(Type::Nominal(nominal)) => pending.extend(nominal.representation),
                Some(Type::Applied { constructor, args }) => {
                    pending.push(TypeId(constructor.0));
                    pending.extend(args.iter().copied());
                }
                Some(Type::Enum(_)) => {
                    if let Some(layout) = self.types.facts.enum_layouts.get(&ty) {
                        pending.extend(
                            layout
                                .variants
                                .iter()
                                .flat_map(|variant| variant.fields.iter().copied()),
                        );
                    }
                }
                Some(Type::Tuple(items)) => pending.extend(items.iter().copied()),
                Some(
                    Type::Array(item)
                    | Type::List(item)
                    | Type::Set(item)
                    | Type::Slice(item)
                    | Type::Option(item)
                    | Type::Trust { inner: item, .. }
                    | Type::Refined { base: item, .. }
                    | Type::Message(item),
                ) => pending.push(*item),
                Some(
                    Type::Map { key, value }
                    | Type::Result {
                        ok: key,
                        err: value,
                    },
                ) => {
                    pending.extend([*key, *value]);
                }
                _ => {}
            }
        }
        self.contains.insert(root, result);
        result
    }

    fn move_place(&mut self, place: Place, span: Span, state: &mut LeaseState) {
        if state.moved.iter().any(|moved| overlaps(moved, &place)) {
            self.moved_uses.insert(span);
        }
        state.moved.insert(place);
    }

    fn restore_bindings(&self, state: &mut LeaseState, declared_here: impl Fn(&SymbolDef) -> bool) {
        let hir = self.context.program();
        state.moved.retain(|place| {
            let AliasTarget::Symbol(symbol) = place.root else {
                return true;
            };
            !hir.symbols
                .get(symbol)
                .is_some_and(|symbol| declared_here(&symbol.def))
        });
    }
}

fn value_type(fact: &SymbolTypeFact) -> Option<TypeId> {
    match fact {
        SymbolTypeFact::Param { ty }
        | SymbolTypeFact::Local { ty, .. }
        | SymbolTypeFact::Value { ty }
        | SymbolTypeFact::TopLevelLet { ty, .. } => Some(*ty),
        _ => None,
    }
}

fn place_of(hir: &HirProgram, mut expr: HirExprId) -> Option<Place> {
    let mut reversed = Vec::new();
    loop {
        match &hir.exprs[expr] {
            HirExpr::Path(path) => {
                let symbol = match &path.resolution {
                    ResolveResult::Resolved(symbol) => *symbol,
                    ResolveResult::PartiallyResolved(path)
                        if path.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
                    {
                        reversed
                            .extend(path.remaining.iter().rev().cloned().map(Projection::Field));
                        path.resolved_prefix?
                    }
                    _ => return None,
                };
                return Some(Place {
                    root: AliasTarget::Symbol(symbol),
                    projections: reversed.into_iter().rev().collect(),
                });
            }
            HirExpr::Field { base, field, .. } => {
                reversed.push(Projection::Field(field.clone()));
                expr = *base;
            }
            HirExpr::Index { base, index, .. } => {
                let projection = match &hir.exprs[*index] {
                    HirExpr::Literal(HirLiteral::Int { text, .. })
                        if text.bytes().all(|b| b.is_ascii_digit()) =>
                    {
                        Projection::ConstIndex(text.trim_start_matches('0').to_owned())
                    }
                    _ => Projection::Index,
                };
                reversed.push(projection);
                expr = *base;
            }
            _ => return None,
        }
    }
}

fn overlaps(a: &Place, b: &Place) -> bool {
    a.root == b.root
        && a.projections.iter().zip(&b.projections).all(|(a, b)| {
            a == b
                || matches!(
                    (a, b),
                    (Projection::Index, Projection::ConstIndex(_))
                        | (Projection::ConstIndex(_), Projection::Index)
                )
        })
}

impl HirAnalysisSemantics for LeaseAnalysis<'_> {
    type Domain = LeaseState;

    fn analysis_context(&self) -> &HirAnalysisContext {
        &self.context
    }

    fn incomplete_facts(&mut self, state: LeaseState) -> LeaseState {
        state
    }

    fn after_expr_control(
        &mut self,
        expr: HirExprId,
        control: Control<LeaseState>,
    ) -> Control<LeaseState> {
        control.with_normal_processed(|state| Control::normal(self.after_expr(expr, state)))
    }

    fn after_expr(&mut self, expr: HirExprId, mut state: LeaseState) -> LeaseState {
        let owned = self
            .types
            .facts
            .expr_types
            .get(&expr)
            .is_some_and(|ty| self.contains_lease(*ty));
        let hir = self.context.revision();
        if Some(expr) == self.captures.tail {
            self.captures.check_return(hir, expr, &state);
        }
        if !owned && !self.captures.is_affine_place(hir, expr, &state) {
            return state;
        }
        let hir = self.context.program();
        let is_projection_base = match self.context.tree_index().expr_owner.get(&expr) {
            Some(HirOwner::Expr(parent)) => matches!(&hir.exprs[*parent],
                HirExpr::Field { base, .. } | HirExpr::Index { base, .. } if *base == expr),
            Some(HirOwner::Stmt(stmt)) => matches!(&hir.stmts[*stmt],
                HirStmt::Assign { target, .. } if *target == expr),
            _ => false,
        };
        if !is_projection_base && let Some(place) = place_of(hir, expr) {
            let span = hir.exprs[expr].span(&hir.blocks);
            self.move_place(place, span, &mut state);
        }
        state
    }

    fn after_stmt(&mut self, stmt: HirStmtId, mut state: LeaseState) -> LeaseState {
        let context = self.context.clone();
        let hir = context.program();
        match &hir.stmts[stmt] {
            HirStmt::Assign {
                target,
                value,
                span,
            } => {
                let typed_lease = self
                    .types
                    .facts
                    .expr_types
                    .get(value)
                    .is_some_and(|ty| self.contains_lease(*ty));
                let mut origins = self.captures.retained(context.revision(), *value, &state);
                if typed_lease
                    && let Some(Place {
                        root: AliasTarget::Symbol(symbol),
                        ..
                    }) = place_of(hir, *value)
                {
                    origins.insert(symbol);
                }
                self.captures.check_store(
                    context.revision(),
                    self.types,
                    stmt,
                    *target,
                    *span,
                    &origins,
                );
                if let Some(place) = place_of(hir, *target) {
                    if let AliasTarget::Symbol(symbol) = place.root {
                        if place.projections.is_empty() {
                            state.captured.insert(symbol, origins.into_iter().collect());
                        } else if !origins.is_empty() {
                            state.captured.entry(symbol).or_default().extend(origins);
                        }
                    }
                    if !place.projections.contains(&Projection::Index) {
                        state.moved.retain(|moved| {
                            moved.root != place.root
                                || !moved.projections.starts_with(&place.projections)
                        });
                    }
                }
            }
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                if let etas_hir::HirPat::Binding { symbol, .. } = hir.pats[*pat] {
                    let origins = self.captures.retained(context.revision(), *value, &state);
                    state.captured.insert(symbol, origins.into_iter().collect());
                }
                self.restore_bindings(
                    &mut state,
                    |def| matches!(def, SymbolDef::Local { binding, .. } if *binding == stmt),
                );
            }
            _ => {}
        }
        state
    }

    fn return_value(
        &mut self,
        _: HirStmtId,
        value: Option<HirExprId>,
        state: LeaseState,
    ) -> LeaseState {
        if let Some(value) = value {
            self.captures
                .check_return(self.context.revision(), value, &state);
        }
        state
    }

    fn begin_match_arm(
        &mut self,
        expr: HirExprId,
        _: HirExprId,
        _: etas_hir::HirPatId,
        mut state: LeaseState,
    ) -> LeaseState {
        use etas_hir::PatternBindingOwner;
        self.restore_bindings(&mut state, |def| {
            matches!(def, SymbolDef::PatternBinding { owner: PatternBindingOwner::MatchArm { expr: owner }, .. } if *owner == expr)
        });
        state
    }

    fn enter_block(&mut self, block: HirBlockId, mut state: LeaseState) -> LeaseState {
        use etas_hir::PatternBindingOwner;
        if let Some(HirOwner::Stmt(stmt)) = self.context.tree_index().block_owner.get(&block)
            && matches!(self.context.program().stmts[*stmt], HirStmt::For { body, .. } if body == block)
        {
            self.restore_bindings(&mut state, |def| {
                matches!(def, SymbolDef::PatternBinding { owner: PatternBindingOwner::For { stmt: owner }, .. } if owner == stmt)
            });
        }
        state
    }

    fn record_shorthand(
        &mut self,
        symbol: SymbolId,
        span: Span,
        mut state: LeaseState,
    ) -> LeaseState {
        if self.captures.is_affine(symbol, &state) {
            self.move_place(Place::new(AliasTarget::Symbol(symbol)), span, &mut state);
        }
        state
    }

    fn perform(
        &mut self,
        _: HirExprId,
        _: &ResolvedActionRef,
        _: &[HirGenericArg],
        _: &[HirArg],
        _: Span,
        state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }

    fn direct_call(
        &mut self,
        _: HirExprId,
        _: HirExprId,
        _: Span,
        state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }

    fn method_call(
        &mut self,
        _: HirExprId,
        _: Span,
        state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }
    fn stage_compose(
        &mut self,
        _: HirExprId,
        _: Span,
        state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }
    fn pipeline(&mut self, _: HirExprId, _: Span, state: LeaseState) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }
    fn try_expr(&mut self, _: HirExprId, _: Span, state: LeaseState) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }
    fn handle_expr(
        &mut self,
        _: HirExprId,
        _: Span,
        state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        AnalysisStep::handled(state)
    }
    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        for symbol in &self.captures.layouts[&expr].captures {
            if self.captures.is_affine(*symbol, &state) {
                self.move_place(Place::new(AliasTarget::Symbol(*symbol)), span, &mut state);
            }
        }
        AnalysisStep::handled(state)
    }
    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: LeaseState,
    ) -> AnalysisStep<LeaseState> {
        self.lambda_boundary(expr, span, state)
    }
}
