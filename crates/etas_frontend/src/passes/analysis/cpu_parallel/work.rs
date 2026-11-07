use std::collections::HashMap;

use etas_hir::{
    ExprView, HirArg, HirExpr, HirItem, HirItemId, HirLiteral, HirStmt, HirTreeView, HirVisitor,
    ResolveResult, StmtView, walk_item,
};

#[derive(Default)]
struct Work {
    local: u64,
    calls: Vec<(HirItemId, u64)>,
    unknown: bool,
}

pub(super) fn estimate(view: &HirTreeView<'_>) -> HashMap<HirItemId, Option<u64>> {
    let mut summaries = HashMap::new();
    for declaration in view.items() {
        if !matches!(declaration.data(), HirItem::Flow(_)) {
            continue;
        }
        let item = declaration.id();
        let mut counter = Counter {
            hir: view.program(),
            work: Work::default(),
            multiplier: 1,
            stack: Vec::new(),
        };
        walk_item(view, item, &mut counter);
        summaries.insert(item, counter.work);
    }
    let mut estimates = HashMap::new();
    loop {
        let resolved = summaries
            .iter()
            .filter_map(|(item, work)| {
                if work.unknown || estimates.contains_key(item) {
                    return None;
                }
                let total = work
                    .calls
                    .iter()
                    .try_fold(work.local, |total, (callee, count)| {
                        estimates
                            .get(callee)
                            .map(|cost: &u64| total.saturating_add(cost.saturating_mul(*count)))
                    })?;
                Some((*item, total))
            })
            .collect::<Vec<_>>();
        if resolved.is_empty() {
            break;
        }
        estimates.extend(resolved);
    }
    summaries
        .keys()
        .map(|item| (*item, estimates.get(item).copied()))
        .collect()
}

struct Counter<'a> {
    hir: &'a etas_hir::HirProgram,
    work: Work,
    multiplier: u64,
    stack: Vec<u64>,
}

pub(super) fn estimate_closure(
    view: &HirTreeView<'_>,
    body: etas_hir::HirLambdaBody,
    items: &HashMap<HirItemId, Option<u64>>,
) -> Option<u64> {
    let mut counter = Counter {
        hir: view.program(),
        work: Work::default(),
        multiplier: 1,
        stack: Vec::new(),
    };
    match body {
        etas_hir::HirLambdaBody::Expr(expr) => etas_hir::walk_expr(view, expr, &mut counter),
        etas_hir::HirLambdaBody::Block(block) => etas_hir::walk_block(view, block, &mut counter),
    }
    if counter.work.unknown {
        return None;
    }
    counter
        .work
        .calls
        .iter()
        .try_fold(counter.work.local, |total, (item, count)| {
            items
                .get(item)
                .copied()
                .flatten()
                .map(|work| total.saturating_add(work.saturating_mul(*count)))
        })
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for Counter<'_> {
    fn enter_stmt(&mut self, stmt: StmtView<'view, 'hir>) {
        self.stack.push(self.multiplier);
        match stmt.data() {
            HirStmt::While { limits, .. }
            | HirStmt::For { limits, .. }
            | HirStmt::Retry { limits, .. } => {
                if let Some(count) = iteration_limit(self.hir, limits) {
                    self.multiplier = self.multiplier.saturating_mul(count);
                } else {
                    self.work.unknown = true;
                }
            }
            _ => {}
        }
        self.work.local = self.work.local.saturating_add(self.multiplier);
    }

    fn exit_stmt(&mut self, _stmt: StmtView<'view, 'hir>) {
        self.multiplier = self.stack.pop().expect("balanced HIR statement visit");
    }

    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        self.work.local = self.work.local.saturating_add(self.multiplier);
        match expr.data() {
            HirExpr::Call { callee, .. } => {
                let HirExpr::Path(path) = &self.hir.exprs[*callee] else {
                    self.work.unknown = true;
                    return;
                };
                // Loop limits are compiler checked constructors, not source calls.
                if path
                    .segments
                    .last()
                    .is_some_and(|part| part.name == "Iterations")
                {
                    return;
                }
                let item = match path.resolution {
                    ResolveResult::Resolved(symbol) => self
                        .hir
                        .canonical_source_symbol(symbol)
                        .and_then(|symbol| self.hir.symbols.get(symbol))
                        .and_then(|symbol| symbol.defining_item),
                    _ => None,
                };
                match item
                    .filter(|item| matches!(self.hir.items.get(*item), Some(HirItem::Flow(_))))
                {
                    Some(item) => self.work.calls.push((item, self.multiplier)),
                    None => self.work.unknown = true,
                }
            }
            HirExpr::MethodCall { .. }
            | HirExpr::SpecMethodCall { .. }
            | HirExpr::Lambda { .. }
            | HirExpr::Pipeline { .. }
            | HirExpr::StageCompose { .. } => {
                self.work.unknown = true;
            }
            _ => {}
        }
    }
}

fn iteration_limit(hir: &etas_hir::HirProgram, limits: &[etas_hir::HirExprId]) -> Option<u64> {
    let [limit] = limits else {
        return None;
    };
    let HirExpr::Call { callee, args, .. } = &hir.exprs[*limit] else {
        return None;
    };
    let HirExpr::Path(path) = &hir.exprs[*callee] else {
        return None;
    };
    let [HirArg::Positional(count)] = args.as_slice() else {
        return None;
    };
    let HirExpr::Literal(HirLiteral::Int { text, .. }) = &hir.exprs[*count] else {
        return None;
    };
    (path.segments.last()?.name == "Iterations")
        .then(|| text.parse().ok())
        .flatten()
}
