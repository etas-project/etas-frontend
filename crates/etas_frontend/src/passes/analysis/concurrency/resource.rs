use std::collections::{HashMap, HashSet};

use etas_hir::{
    ExprView, HirArg, HirExpr, HirExprId, HirItem, HirItemId, HirLambdaBody, HirProgram, HirStmt,
    HirVisitor, ResolveResult, SymbolDef, walk_block_tree, walk_expr_tree,
};
use etas_std::{IntrinsicMemoryAccess, StdRegistry, intrinsic::runtime::*};
use etas_types::TypeFacts;

use super::{
    ResourceGuardCall, ResourceGuardCallablePlan, ResourceValueSelector, RuntimeResourceSelector,
};

pub(super) fn analyze(
    view: &etas_hir::HirTreeView<'_>,
    types: &TypeFacts,
    standard: &StdRegistry,
    effects: &etas_effects::EffectFacts,
) -> (
    HashMap<HirItemId, ResourceGuardCallablePlan>,
    HashMap<HirExprId, ResourceGuardCallablePlan>,
) {
    let hir = view.program();
    // Index writes once so admission never freezes a selector that the task
    // changes before using it. Etas parameters and lets can be assigned today.
    let mutated = hir
        .stmts
        .iter()
        .filter_map(|(_, stmt)| {
            let HirStmt::Assign { target, .. } = stmt else {
                return None;
            };
            assigned_symbol(hir, *target)
        })
        .collect::<HashSet<_>>();
    // A nominally non-Memory action can be redirected by a Source handler.
    // Only trivial resumptions and task orchestration are transparent here;
    // other handler bodies still need a handler-aware resource proof.
    let opaque_actions = hir
        .handler_arms
        .iter()
        .filter_map(|(_, arm)| {
            let mut scanner = HandlerSafety {
                types,
                safe: true,
                nested: 0,
            };
            walk_block_tree(view, arm.body, &mut scanner);
            if scanner.safe {
                return None;
            }
            let ResolveResult::Resolved(symbol) = arm.action.action_symbol else {
                return None;
            };
            Some(hir.canonical_source_symbol(symbol).unwrap_or(symbol))
        })
        .collect::<HashSet<_>>();
    let mut callables = HashMap::new();
    let mut closures = HashMap::new();
    for item in view.items() {
        let (parameters, block) = match item.data() {
            HirItem::Flow(flow) => (flow.params.clone(), flow.body.block()),
            HirItem::Tool(tool) => match &tool.body {
                etas_hir::HirToolBody::Source(body) => (tool.params.clone(), body.block()),
                _ => continue,
            },
            HirItem::Agent(agent) => match agent.body.block() {
                Some(block) => (agent.params.clone(), block),
                None => continue,
            },
            _ => continue,
        };
        let mut scanner = Scanner::new(
            hir,
            types,
            standard,
            effects,
            &mutated,
            &opaque_actions,
            parameters,
        );
        walk_block_tree(view, block, &mut scanner);
        callables.insert(item.id(), scanner.plan);
        let mut pending = scanner.closures;
        while let Some(expr) = pending.pop() {
            if closures.contains_key(&expr) {
                continue;
            }
            let HirExpr::Lambda { params, body, .. } = &hir.exprs[expr] else {
                unreachable!()
            };
            let mut scanner = Scanner::new(
                hir,
                types,
                standard,
                effects,
                &mutated,
                &opaque_actions,
                params.clone(),
            );
            match body {
                HirLambdaBody::Block(block) => walk_block_tree(view, *block, &mut scanner),
                HirLambdaBody::Expr(expr) => walk_expr_tree(view, *expr, &mut scanner),
            }
            pending.extend(scanner.closures);
            closures.insert(expr, scanner.plan);
        }
    }
    (callables, closures)
}

struct Scanner<'a> {
    hir: &'a HirProgram,
    types: &'a TypeFacts,
    standard: &'a StdRegistry,
    effects: &'a etas_effects::EffectFacts,
    mutated: &'a HashSet<etas_hir::SymbolId>,
    opaque_actions: &'a HashSet<etas_hir::SymbolId>,
    plan: ResourceGuardCallablePlan,
    closures: Vec<HirExprId>,
    nested: usize,
}

impl<'a> Scanner<'a> {
    fn new(
        hir: &'a HirProgram,
        types: &'a TypeFacts,
        standard: &'a StdRegistry,
        effects: &'a etas_effects::EffectFacts,
        mutated: &'a HashSet<etas_hir::SymbolId>,
        opaque_actions: &'a HashSet<etas_hir::SymbolId>,
        parameters: Vec<etas_hir::SymbolId>,
    ) -> Self {
        Self {
            hir,
            types,
            standard,
            effects,
            mutated,
            opaque_actions,
            plan: ResourceGuardCallablePlan {
                parameters,
                complete: true,
                ..Default::default()
            },
            closures: Vec::new(),
            nested: 0,
        }
    }

    fn may_access_memory(&self, expr: HirExprId) -> bool {
        if self
            .effects
            .performed_actions
            .get(&expr)
            .and_then(|fact| fact.action_symbol)
            .is_some_and(|symbol| {
                self.opaque_actions
                    .contains(&self.hir.canonical_source_symbol(symbol).unwrap_or(symbol))
            })
        {
            return true;
        }
        let Some(summary) = self
            .effects
            .performed_actions
            .get(&expr)
            .map(|fact| &fact.summary)
            .or_else(|| self.effects.expr_effects.get(&expr))
        else {
            return true;
        };
        if summary.requested_actions.open.is_some() || summary.escaping_effects.open.is_some() {
            return true;
        }
        let is_memory = |effect: &etas_effects::Effect| match effect {
            etas_effects::Effect::Action(action) => action.tag == etas_effects::MEMORY_TAG,
            etas_effects::Effect::AppliedAction(action) => {
                action.action.tag == etas_effects::MEMORY_TAG
            }
            etas_effects::Effect::Tag(tag) | etas_effects::Effect::Applied { tag, .. } => {
                *tag == etas_effects::MEMORY_TAG
            }
            etas_effects::Effect::Var(_) => true,
            _ => false,
        };
        summary
            .requested_actions
            .effects
            .iter()
            .chain(summary.default_actions.effects.iter())
            .chain(summary.escaping_effects.effects.iter())
            .any(is_memory)
            || summary.action_trace.action_set().iter().any(is_memory)
    }

    fn call(&mut self, expr: HirExprId) {
        if self.types.selected_method(expr).is_some_and(|method| {
            method.signature.effects.is_some()
                && method
                    .signature
                    .effects
                    .iter()
                    .chain(&method.signature.requested_actions)
                    .all(|row| row.tail.is_none() && row.effects.is_empty())
        }) {
            return;
        }
        if let Some(fact) = self.types.std_intrinsic_calls.get(&expr) {
            let descriptor = self
                .standard
                .lookup_qualified(&fact.qualified_path)
                .and_then(|symbol| symbol.intrinsic.as_ref())
                .expect("checked standard intrinsic");
            if descriptor.memory_access == IntrinsicMemoryAccess::None {
                if fact
                    .qualified_path
                    .get(1)
                    .is_some_and(|part| part == "task")
                {
                    return;
                }
                let may_access_memory = fact
                    .signature
                    .effects
                    .iter()
                    .chain(&fact.signature.requested_actions)
                    .any(|row| {
                        row.tail.is_some()
                            || row.effects.iter().any(|effect| {
                                effect.name == "Memory" || effect.name.starts_with("Memory.")
                            })
                    });
                if may_access_memory {
                    self.plan.complete = false;
                    self.plan.opaque_memory = true;
                }
                return;
            }
            if descriptor.memory_access == IntrinsicMemoryAccess::WriteFirstArgIntent {
                self.plan.complete = false;
                self.plan.opaque_memory |= self.may_access_memory(expr);
                return;
            }
            let Some((store, args)) = memory_arguments(self.hir, expr) else {
                self.plan.complete = false;
                self.plan.opaque_memory |= self.may_access_memory(expr);
                return;
            };
            let keyed = matches!(
                fact.intrinsic,
                MEMORY_GET
                    | MEMORY_GET_ENTRY
                    | MEMORY_CONTAINS
                    | MEMORY_PUT
                    | MEMORY_PUT_VERSIONED
                    | MEMORY_INSERT
                    | MEMORY_DELETE
                    | MEMORY_DELETE_VERSIONED
            );
            // Higher-order memory methods need their callback's resource plan.
            // Until that is available, preserve the serial execution path.
            self.plan.complete &= keyed
                || matches!(
                    fact.intrinsic,
                    MEMORY_KEYS | MEMORY_PAGE | MEMORY_SCAN | MEMORY_CLEAR
                );
            let key = keyed
                .then(|| args[0])
                .filter(|expr| match &self.hir.exprs[*expr] {
                    HirExpr::Literal(_) => true,
                    HirExpr::Path(path) => {
                        matches!(path.resolution, ResolveResult::Resolved(_))
                            && read_only(self.hir, *expr, self.mutated)
                    }
                    _ => false,
                });
            self.plan.complete &= match &store {
                ResourceValueSelector::Expr(expr) => read_only(self.hir, *expr, self.mutated),
                ResourceValueSelector::SymbolPath { symbol, .. } => {
                    immutable(self.hir, *symbol, self.mutated)
                }
            };
            // A dynamic or mutated key cannot be frozen at admission time. Guard
            // the entire Store instead of broadening the whole invocation.
            self.plan
                .resources
                .push(RuntimeResourceSelector { store, key });
            return;
        }
        let HirExpr::Call { callee, args, .. } = &self.hir.exprs[expr] else {
            self.plan.complete = false;
            self.plan.opaque_memory |= self.may_access_memory(expr);
            return;
        };
        let HirExpr::Path(path) = &self.hir.exprs[*callee] else {
            self.plan.complete = false;
            self.plan.opaque_memory |= self.may_access_memory(expr);
            return;
        };
        let qualified = path
            .segments
            .iter()
            .map(|segment| segment.name.clone())
            .collect::<Vec<_>>();
        if self.standard.lookup_qualified(&qualified).is_some_and(|symbol| {
            symbol.kind == etas_std::StdSymbolKind::Constructor || matches!(
                &symbol.decl,
                etas_std::StdDecl::Flow(flow) if flow.public_effects.is_empty()
                    && flow.public_effect_tail.is_none() && flow.requested_actions.is_empty()
                    && flow.requested_action_tail.is_none()
            )
        }) { return; }
        let ResolveResult::Resolved(symbol) = path.resolution else {
            self.plan.complete = false;
            self.plan.opaque_memory |= self.may_access_memory(expr);
            return;
        };
        if self
            .hir
            .symbols
            .get(self.hir.canonical_source_symbol(symbol).unwrap_or(symbol))
            .is_some_and(|symbol| match &symbol.def {
                SymbolDef::EnumVariant { .. } | SymbolDef::ExternalEnumVariant { .. } => true,
                SymbolDef::ImportAlias { path, .. } => self
                    .standard
                    .lookup_qualified(path)
                    .is_some_and(|symbol| symbol.kind == etas_std::StdSymbolKind::Constructor),
                _ => false,
            })
        {
            return;
        }
        let item = self
            .hir
            .canonical_source_symbol(symbol)
            .and_then(|symbol| self.hir.symbols.get(symbol))
            .and_then(|symbol| match symbol.def {
                SymbolDef::Item { item } => Some(item),
                _ => None,
            });
        if let Some(item) = item.filter(|item| {
            matches!(
                self.hir.items.get(*item),
                Some(HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_))
            )
        }) {
            let arguments = args
                .iter()
                .map(argument)
                .map(|expr| read_only(self.hir, expr, self.mutated).then_some(expr))
                .collect::<Vec<_>>();
            // Only arguments used as resource selectors need to be frozen.
            // The runtime resolves those lazily; a nested Command.read result,
            // for example, must never be evaluated during task admission.
            self.plan.calls.push(ResourceGuardCall {
                callable: item,
                arguments,
            });
        } else {
            self.plan.complete = false;
            self.plan.opaque_memory |= self.may_access_memory(expr);
        }
    }
}

struct HandlerSafety<'a> {
    types: &'a TypeFacts,
    safe: bool,
    nested: usize,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for HandlerSafety<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if matches!(expr.data(), HirExpr::Lambda { .. }) {
            self.nested += 1;
        }
        if self.nested != 0 {
            return;
        }
        match expr.data() {
            HirExpr::Call { .. } | HirExpr::MethodCall { .. } | HirExpr::SpecMethodCall { .. } => {
                self.safe &= self
                    .types
                    .std_intrinsic_calls
                    .get(&expr.id())
                    .is_some_and(|fact| {
                        fact.qualified_path
                            .get(1)
                            .is_some_and(|part| part == "task")
                    });
            }
            HirExpr::Perform { .. } | HirExpr::Infer { .. } => self.safe = false,
            _ => {}
        }
    }
    fn exit_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if matches!(expr.data(), HirExpr::Lambda { .. }) {
            self.nested -= 1;
        }
    }
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for Scanner<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if matches!(expr.data(), HirExpr::Lambda { .. }) {
            self.closures.push(expr.id());
            self.nested += 1;
        }
        if self.nested != 0 {
            return;
        }
        match expr.data() {
            HirExpr::Call { .. } | HirExpr::MethodCall { .. } | HirExpr::SpecMethodCall { .. } => {
                self.call(expr.id())
            }
            HirExpr::Perform { .. } | HirExpr::Infer { .. } => {
                if self.may_access_memory(expr.id()) {
                    self.plan.complete = false;
                    self.plan.opaque_memory = true;
                }
            }
            _ => {}
        }
    }
    fn exit_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if matches!(expr.data(), HirExpr::Lambda { .. }) {
            self.nested -= 1;
        }
    }
}

fn argument(arg: &HirArg) -> HirExprId {
    match arg {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
    }
}

fn memory_arguments(
    hir: &HirProgram,
    expr: HirExprId,
) -> Option<(ResourceValueSelector, Vec<HirExprId>)> {
    let (store, args) = match &hir.exprs[expr] {
        HirExpr::MethodCall { receiver, args, .. } => (
            ResourceValueSelector::Expr(*receiver),
            args.iter().map(argument).collect(),
        ),
        HirExpr::Call { callee, args, .. } => match &hir.exprs[*callee] {
            HirExpr::Field { base, .. } => (
                ResourceValueSelector::Expr(*base),
                args.iter().map(argument).collect(),
            ),
            HirExpr::Path(path) => match &path.resolution {
                ResolveResult::PartiallyResolved(partial) => (
                    ResourceValueSelector::SymbolPath {
                        symbol: partial.resolved_prefix?,
                        fields: partial.remaining[..partial.remaining.len() - 1].to_vec(),
                    },
                    args.iter().map(argument).collect(),
                ),
                _ => (
                    ResourceValueSelector::Expr(argument(args.first()?)),
                    args[1..].iter().map(argument).collect(),
                ),
            },
            _ => return None,
        },
        _ => return None,
    };
    Some((store, args))
}

fn immutable(
    hir: &HirProgram,
    symbol: etas_hir::SymbolId,
    mutated: &HashSet<etas_hir::SymbolId>,
) -> bool {
    if mutated.contains(&symbol) {
        return false;
    }
    match &hir
        .symbols
        .get(symbol)
        .expect("resolved selector symbol")
        .def
    {
        SymbolDef::Local {
            binding,
            initializer,
            ..
        } => {
            matches!(hir.stmts[*binding], HirStmt::Let { .. })
                && initializer.is_none_or(|expr| read_only(hir, expr, mutated))
        }
        SymbolDef::Param { .. } | SymbolDef::TopLevelLet { .. } => true,
        _ => false,
    }
}

fn read_only(hir: &HirProgram, expr: HirExprId, mutated: &HashSet<etas_hir::SymbolId>) -> bool {
    match &hir.exprs[expr] {
        HirExpr::Literal(_) => true,
        HirExpr::Path(path) => match &path.resolution {
            ResolveResult::Resolved(symbol) => immutable(hir, *symbol, mutated),
            ResolveResult::PartiallyResolved(partial) => partial
                .resolved_prefix
                .is_some_and(|symbol| immutable(hir, symbol, mutated)),
            _ => false,
        },
        HirExpr::Field { base, .. } => read_only(hir, *base, mutated),
        _ => false,
    }
}

fn assigned_symbol(hir: &HirProgram, mut expr: HirExprId) -> Option<etas_hir::SymbolId> {
    loop {
        match &hir.exprs[expr] {
            HirExpr::Path(path) => {
                return match &path.resolution {
                    ResolveResult::Resolved(symbol) => Some(*symbol),
                    ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix,
                    _ => None,
                };
            }
            HirExpr::Field { base, .. }
            | HirExpr::Index { base, .. }
            | HirExpr::Slice { base, .. } => expr = *base,
            _ => return None,
        }
    }
}
