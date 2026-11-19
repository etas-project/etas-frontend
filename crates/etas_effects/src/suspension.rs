//! Suspension is a property of checked execution, independent of effect rows
//! and determinism. Missing callback/import behavior cannot establish a proof.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use etas_hir::{
    HirExpr, HirExprId, HirItem, HirRevision, HirStmt, HirTreeChild as Child, ResolveResult,
    SymbolDef,
};
use etas_hir_analysis::{HirAnalysisContext, interprocedural::HirAnalysisBody};
use etas_types::{SymbolTypeFact, Type, TypeOutput, method::MethodTarget};

use crate::{EffectFacts, EffectUnit};

mod instances;
mod iteration;
mod values;
use instances::Instance;

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct SuspensionSummary {
    pub may_suspend: bool,
    pub unknown: bool,
}

impl SuspensionSummary {
    pub const IMMEDIATE: Self = Self {
        may_suspend: false,
        unknown: false,
    };
    pub const SUSPENDING: Self = Self {
        may_suspend: true,
        unknown: false,
    };
    pub const UNKNOWN: Self = Self {
        may_suspend: false,
        unknown: true,
    };

    pub fn proven_non_suspending(self) -> bool {
        !self.may_suspend && !self.unknown
    }

    fn join(&mut self, other: Self) -> bool {
        let before = *self;
        self.may_suspend |= other.may_suspend;
        self.unknown |= other.unknown;
        *self != before
    }
}

#[derive(Default)]
struct Node {
    local: SuspensionSummary,
    calls: BTreeSet<Instance>,
}

#[derive(Clone, Copy)]
struct CallContext<'a> {
    hir: &'a HirRevision,
    types: &'a TypeOutput,
    effects: &'a EffectFacts,
    registry: &'a etas_std::StdRegistry,
    imported: &'a BTreeMap<etas_code::CallableId, SuspensionSummary>,
    bindings: &'a [(etas_types::TypeId, etas_types::TypeId)],
    specs: &'a etas_types::SpecFacts,
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
    environment: Option<crate::facts::CallableEnvironmentId>,
    payload_shapes: &'a crate::infer::semantics::latent_shape::LatentTypeShapes,
    values: &'a instances::ValueBindings,
}

/// Infer a monotone summary over actual source calls, including recursive
/// components. Recursion alone does not imply suspension (or termination).
pub fn analyze(
    hir: &HirRevision,
    types: &TypeOutput,
    effects: &EffectFacts,
    registry: &etas_std::StdRegistry,
    imported: &BTreeMap<etas_code::CallableId, SuspensionSummary>,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<BTreeMap<EffectUnit, SuspensionSummary>, etas_utils::PipelineInterruption> {
    let context = HirAnalysisContext::new(hir);
    let view = hir.view();
    let specs = etas_types::SpecFacts::from(types.facts.as_ref());
    let payload_shapes = crate::infer::semantics::latent_shape::LatentTypeShapes::build(types);
    let mut graph = BTreeMap::new();
    let mut graph_size = 0usize;
    let mut pending_instances = effects
        .unit_effects
        .keys()
        .copied()
        .map(Instance::declaration)
        .collect::<Vec<_>>();
    let frozen_owners: BTreeSet<_> = effects
        .frozen_callable_graphs
        .values()
        .map(|graph| graph.owner)
        .collect();
    while let Some(instance) = pending_instances.pop() {
        checkpoint(control)?;
        if graph.contains_key(&instance) {
            continue;
        }
        graph_size = graph_size.saturating_add(
            1 + instance.bindings.len()
                + instance
                    .values
                    .values()
                    .map(|origins| 1 + origins.len())
                    .sum::<usize>(),
        );
        if graph_size > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                reason: "suspension instantiation graph exceeds compiler node budget".into(),
            });
        }
        let unit = instance.unit;
        let calls = CallContext {
            hir,
            types,
            effects,
            registry,
            imported,
            bindings: &instance.bindings,
            specs: &specs,
            control,
            environment: instance.environment,
            payload_shapes: &payload_shapes,
            values: &instance.values,
        };
        let mut node = Node::default();
        let owner = match unit {
            EffectUnit::Item(owner)
            | EffectUnit::AnonymousFlow { owner, .. }
            | EffectUnit::HandlerArm { owner, .. } => owner,
            EffectUnit::ParameterDefault { owner, .. } => owner,
        };
        if frozen_owners.contains(&owner) {
            node.local = SuspensionSummary::UNKNOWN;
        }
        if matches!(unit, EffectUnit::Item(item) if matches!(hir.items[item], HirItem::Tool(_) | HirItem::Agent(_)))
        {
            node.local = SuspensionSummary::UNKNOWN;
        }
        let mut pending = match unit.analysis_body_with_context(&context) {
            HirAnalysisBody::Block(block) => vec![Child::Block(block)],
            HirAnalysisBody::Expr(expr) => vec![Child::Expr(expr)],
            HirAnalysisBody::HandlerArm(arm) => vec![Child::Block(hir.handler_arms[arm].body)],
            HirAnalysisBody::External | HirAnalysisBody::Missing => {
                let execution = match unit {
                    EffectUnit::Item(item) => match &hir.items[item] {
                        HirItem::Flow(etas_hir::HirFlowDecl {
                            body: etas_hir::HirFlowBody::Runtime { role, .. },
                            ..
                        }) => Some(*role),
                        _ => None,
                    },
                    _ => None,
                };
                node.local.join(match execution {
                    Some(
                        etas_std::StdExecutionRole::Abort
                        | etas_std::StdExecutionRole::CaptureData
                        | etas_std::StdExecutionRole::StorageContext
                        | etas_std::StdExecutionRole::StorageNonce
                        | etas_std::StdExecutionRole::StreamClock
                        | etas_std::StdExecutionRole::CurrentSession
                        | etas_std::StdExecutionRole::MessageContextCapture
                        | etas_std::StdExecutionRole::MessageContextRecord
                        | etas_std::StdExecutionRole::PayloadConvert
                        | etas_std::StdExecutionRole::ParseBody
                        | etas_std::StdExecutionRole::AgentWithBody
                        | etas_std::StdExecutionRole::CallableWithBody
                        | etas_std::StdExecutionRole::ToolReference
                        | etas_std::StdExecutionRole::TaskStart
                        | etas_std::StdExecutionRole::TaskCancel,
                    ) => SuspensionSummary::IMMEDIATE,
                    Some(
                        etas_std::StdExecutionRole::Checkpoint
                        | etas_std::StdExecutionRole::Requeue
                        | etas_std::StdExecutionRole::Park
                        | etas_std::StdExecutionRole::ScopeEnter
                        | etas_std::StdExecutionRole::TaskNextCompletion,
                    ) => SuspensionSummary::SUSPENDING,
                    None => SuspensionSummary::UNKNOWN,
                });
                Vec::new()
            }
        };
        while let Some(child) = pending.pop() {
            checkpoint(control)?;
            match child {
                Child::Block(block) => {
                    let block = view.block(block).unwrap();
                    pending.extend(block.statements().map(|stmt| Child::Stmt(stmt.id())));
                    pending.extend(block.final_expr().map(|expr| Child::Expr(expr.id())));
                }
                Child::Stmt(stmt) => {
                    let stmt = view.stmt(stmt).unwrap();
                    match stmt.data() {
                        HirStmt::For { iter, .. } => {
                            iteration::begin(calls, *iter, &mut node);
                        }
                        HirStmt::Resume { .. } => {
                            node.local.join(SuspensionSummary::UNKNOWN);
                        }
                        HirStmt::Error { .. } => {
                            node.local.join(SuspensionSummary::UNKNOWN);
                        }
                        _ => {}
                    }
                    pending.extend(stmt.children());
                }
                Child::Expr(expr) => {
                    let expression = view.expr(expr).unwrap();
                    match expression.data() {
                        HirExpr::Call { callee, .. } => {
                            call(calls, expr, Some(*callee), &mut node);
                        }
                        HirExpr::MethodCall { .. } | HirExpr::SpecMethodCall { .. } => {
                            call(calls, expr, None, &mut node);
                        }
                        HirExpr::Perform { .. } if terminating_error(effects, expr) => {
                            // The fixed Error ABI cannot resume this invocation.
                            // An escaping raise releases update access before an
                            // outer handler runs. Internal scopes remain unknown.
                        }
                        HirExpr::Perform { .. } | HirExpr::Infer { .. } => {
                            // A handled/default-handled action can still suspend.
                            // Handler-specific discharge is not inferred from ![].
                            node.local.join(SuspensionSummary::SUSPENDING);
                        }
                        HirExpr::Pipeline { .. } => {
                            // Stages invoke callables without an explicit Call
                            // expression. They need their own target proof.
                            node.local.join(SuspensionSummary::UNKNOWN);
                        }
                        HirExpr::StageCompose { stages, .. } => {
                            for stage in stages {
                                pending.extend(stage.limits.iter().copied().map(Child::Expr));
                            }
                        }
                        HirExpr::Index { base, index, .. } => {
                            // Checked indexing either returns or raises the same
                            // non-resumable Error ABI; Try capture is synchronous.
                            if types.facts.checked_index_errors.contains_key(&expr)
                                && !primitive_index(calls, *base, *index)
                            {
                                node.local.join(SuspensionSummary::UNKNOWN);
                            }
                        }
                        HirExpr::Handle { .. } => {
                            node.local.join(SuspensionSummary::UNKNOWN);
                        }
                        HirExpr::Match { arms, .. } => {
                            // Arm bodies are separately indexed HIR units, but
                            // execute here. A lambda is only created here.
                            pending.extend(arms.iter().map(|arm| match arm.body {
                                etas_hir::HirMatchArmBody::Expr(expr) => Child::Expr(expr),
                                etas_hir::HirMatchArmBody::Block(block) => Child::Block(block),
                            }));
                        }
                        HirExpr::Error { .. } => {
                            node.local.join(SuspensionSummary::UNKNOWN);
                        }
                        _ => {}
                    }
                    pending.extend(expression.children());
                }
                Child::HandlerArm(_) | Child::Pattern(_) => {}
            }
        }
        if let Some(cursor) = instance.cursor {
            iteration::successors(calls, unit, cursor, &mut node);
        }
        pending_instances.extend(
            node.calls
                .iter()
                .filter(|callee| !graph.contains_key(*callee))
                .cloned(),
        );
        graph.insert(instance, node);
    }

    let mut summaries: BTreeMap<_, _> = graph
        .iter()
        .map(|(unit, node)| (unit.clone(), node.local))
        .collect();
    let mut callers: BTreeMap<Instance, BTreeSet<Instance>> = BTreeMap::new();
    for (unit, node) in &graph {
        for callee in &node.calls {
            callers
                .entry(callee.clone())
                .or_default()
                .insert(unit.clone());
        }
    }
    let mut queue: VecDeque<_> = summaries.keys().cloned().collect();
    let mut queued: BTreeSet<_> = summaries.keys().cloned().collect();
    while let Some(unit) = queue.pop_front() {
        checkpoint(control)?;
        queued.remove(&unit);
        let mut next = graph[&unit].local;
        for callee in &graph[&unit].calls {
            next.join(
                summaries
                    .get(callee)
                    .copied()
                    .unwrap_or(SuspensionSummary::UNKNOWN),
            );
        }
        if summaries.get_mut(&unit).unwrap().join(next) {
            for caller in callers.get(&unit).into_iter().flatten() {
                if queued.insert(caller.clone()) {
                    queue.push_back(caller.clone());
                }
            }
        }
    }
    Ok(effects
        .unit_effects
        .keys()
        .map(|unit| (*unit, summaries[&Instance::declaration(*unit)]))
        .collect())
}

fn checkpoint(
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<(), etas_utils::PipelineInterruption> {
    if let Some(control) = control {
        control.checkpoint()?;
    }
    Ok(())
}

fn terminating_error(effects: &EffectFacts, expr: HirExprId) -> bool {
    effects.performed_actions.get(&expr).is_some_and(|fact| {
        fact.action == "raise"
            && match &fact.invocation {
                crate::Effect::Error(_) => true,
                crate::Effect::Action(action) => action.tag == crate::ERROR_TAG,
                crate::Effect::AppliedAction(action) => action.action.tag == crate::ERROR_TAG,
                _ => false,
            }
    })
}

fn primitive_index(context: CallContext<'_>, base: HirExprId, index: HirExprId) -> bool {
    let expression_type =
        |expr| instances::expression_type(context, expr).and_then(|ty| context.types.store.get(ty));
    let primitive_integer = |ty: &Type| {
        matches!(ty, Type::Primitive(p) if matches!(p,
        etas_types::PrimitiveType::I8 | etas_types::PrimitiveType::I16 | etas_types::PrimitiveType::I32 |
        etas_types::PrimitiveType::I64 | etas_types::PrimitiveType::I128 | etas_types::PrimitiveType::ISize |
        etas_types::PrimitiveType::U8 | etas_types::PrimitiveType::U16 | etas_types::PrimitiveType::U32 |
        etas_types::PrimitiveType::U64 | etas_types::PrimitiveType::U128 | etas_types::PrimitiveType::USize))
    };
    matches!(
        expression_type(base),
        Some(
            Type::Array(_)
                | Type::List(_)
                | Type::Slice(_)
                | Type::Primitive(
                    etas_types::PrimitiveType::Bytes | etas_types::PrimitiveType::String
                )
        )
    ) && expression_type(index).is_some_and(primitive_integer)
}

fn call(context: CallContext<'_>, call: HirExprId, callee: Option<HirExprId>, node: &mut Node) {
    let CallContext {
        hir,
        types,
        registry,
        imported,
        ..
    } = context;
    if callee.is_some_and(|callee| {
        matches!(source_target(hir, types, callee), Some(Target::Constructor))
            || native_constructor(context, callee)
    }) {
        return;
    }
    if let Some(symbol) = types.facts.source_flow_calls.get(&call) {
        if let Some(SymbolDef::Item { item }) = hir.symbols.get(*symbol).map(|symbol| &symbol.def) {
            source_call(context, call, EffectUnit::Item(*item), node);
        } else {
            node.local.join(SuspensionSummary::UNKNOWN);
        }
        return;
    }
    if let Some(selected) = types.facts.library_flow_calls.get(&call) {
        node.local.join(
            imported
                .get(&selected.callable)
                .copied()
                .unwrap_or(SuspensionSummary::UNKNOWN),
        );
        return;
    }
    if let Some(method) = types.facts.checked_method_calls.get(&call) {
        match &method.target {
            MethodTarget::Source { callable, .. } => {
                source_call(context, call, EffectUnit::Item(*callable), node);
                return;
            }
            MethodTarget::Agent { .. } => {
                node.local.join(SuspensionSummary::SUSPENDING);
                return;
            }
            MethodTarget::External { identity } => {
                node.local.join(
                    imported
                        .get(&etas_code::CallableId::method(identity.clone()))
                        .copied()
                        .unwrap_or(SuspensionSummary::UNKNOWN),
                );
                return;
            }
            MethodTarget::BoundSpec { member, .. } => {
                if let Some(witness) = instances::witness(context, method, member) {
                    match witness {
                        instances::Witness::Source(instance) => {
                            node.calls.insert(instance);
                        }
                        instances::Witness::External(identity) => {
                            node.local.join(
                                imported
                                    .get(&identity)
                                    .copied()
                                    .unwrap_or(SuspensionSummary::UNKNOWN),
                            );
                        }
                    }
                } else {
                    node.local.join(SuspensionSummary::UNKNOWN);
                }
                return;
            }
            MethodTarget::Standard { .. } => {}
        }
    }
    if let Some(callee) = callee {
        if let HirExpr::Path(path) = &hir.exprs[callee]
            && let ResolveResult::Resolved(symbol) = path.resolution
        {
            let canonical = hir.canonical_source_symbol(symbol).unwrap_or(symbol);
            if let Some(definition) = types
                .facts
                .external_definition_bindings
                .get(&symbol)
                .or_else(|| types.facts.external_definition_bindings.get(&canonical))
            {
                let summary = etas_code::CallableId::declaration(definition.clone())
                    .ok()
                    .and_then(|id| imported.get(&id))
                    .copied()
                    .unwrap_or(SuspensionSummary::UNKNOWN);
                node.local.join(summary);
                return;
            }
        }
    }
    if let Some(native) = types.facts.std_intrinsic_calls.get(&call) {
        let descriptor = registry.intrinsic(etas_std::StdIntrinsicId(native.intrinsic));
        let source = registry
            .lookup_qualified(&native.qualified_path)
            .is_some_and(|symbol| {
                matches!(
                    symbol.implementation,
                    Some(
                        etas_std::StdImplementationRef::Source { .. }
                            | etas_std::StdImplementationRef::Artifact { .. }
                    )
                )
            });
        let summary = match descriptor.map(|descriptor| descriptor.dispatch) {
            Some(etas_std::IntrinsicDispatch::PureKernel) if !source => {
                SuspensionSummary::IMMEDIATE
            }
            Some(etas_std::IntrinsicDispatch::Host) => SuspensionSummary::SUSPENDING,
            _ => SuspensionSummary::UNKNOWN,
        };
        node.local.join(summary);
        return;
    }
    if let Some(sources) = context.effects.dynamic_call_sources.get(&call) {
        values::call(context, call, sources, node);
        return;
    }
    if let Some(targets) = context.effects.native_call_targets.get(&call) {
        for target in targets {
            source_call(context, call, *target, node);
        }
        return;
    }
    if let Some(callee) = callee {
        if let Some(target) = source_target(hir, types, callee) {
            match target {
                Target::Unit(unit) => source_call(context, call, unit, node),
                Target::Constructor => {}
            }
            return;
        }
    }
    node.local.join(SuspensionSummary::UNKNOWN);
}

pub(crate) fn imported_contracts<'a>(
    contracts: impl Iterator<Item = &'a crate::ExternalCallableContract>,
) -> BTreeMap<etas_code::CallableId, SuspensionSummary> {
    let mut result = BTreeMap::new();
    for contract in contracts {
        let summary = contract
            .header
            .analysis
            .suspension
            .map(|info| SuspensionSummary {
                may_suspend: info.may_suspend,
                unknown: info.unknown,
            })
            .unwrap_or(SuspensionSummary::UNKNOWN);
        result
            .entry(contract.header.id.clone())
            .or_insert(SuspensionSummary::IMMEDIATE)
            .join(summary);
    }
    result
}

enum Target {
    Unit(EffectUnit),
    Constructor,
}

fn native_constructor(context: CallContext<'_>, callee: HirExprId) -> bool {
    let HirExpr::Path(path) = &context.hir.exprs[callee] else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = context.hir.canonical_source_symbol(symbol) else {
        return false;
    };
    if etas_types::facts::enums::standard_limit_constructor(context.hir, context.registry, symbol)
        .is_some()
    {
        return true;
    }
    let Some(SymbolDef::ImportAlias { path, .. }) =
        context.hir.symbols.get(symbol).map(|symbol| &symbol.def)
    else {
        return false;
    };
    context.registry.lookup_qualified(path).is_some_and(|symbol| {
        symbol.kind == etas_std::StdSymbolKind::Constructor
            && symbol.intrinsic.as_ref().is_some_and(|intrinsic| {
                intrinsic.dispatch == etas_std::IntrinsicDispatch::PureKernel
                    && matches!(symbol.implementation,
                        Some(etas_std::StdImplementationRef::Primitive { intrinsic: id }) if id == intrinsic.id)
            })
    })
}

fn source_call(context: CallContext<'_>, call: HirExprId, unit: EffectUnit, node: &mut Node) {
    if context
        .effects
        .native_call_targets
        .get(&call)
        .is_some_and(|targets| targets.len() == 1 && targets.contains(&unit))
    {
        if let Some(instance) = instances::source(context, call, unit) {
            node.calls.insert(instance);
        } else {
            node.local.join(SuspensionSummary::UNKNOWN);
        }
    } else {
        node.local.join(SuspensionSummary::UNKNOWN);
    }
}

fn source_target(hir: &HirRevision, types: &TypeOutput, mut expr: HirExprId) -> Option<Target> {
    let mut seen = BTreeSet::new();
    while seen.insert(expr) {
        match &hir.exprs[expr] {
            HirExpr::Lambda { body, .. } => {
                let owner = *hir.index().expr_item.get(&expr)?;
                let body = match body {
                    etas_hir::HirLambdaBody::Expr(expr) => {
                        crate::infer::unit::EffectAnonymousFlowBody::Expr(*expr)
                    }
                    etas_hir::HirLambdaBody::Block(block) => {
                        crate::infer::unit::EffectAnonymousFlowBody::Block(*block)
                    }
                };
                return Some(Target::Unit(EffectUnit::AnonymousFlow {
                    owner,
                    value: expr,
                    body,
                }));
            }
            HirExpr::Path(path) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return None;
                };
                let symbol = hir.canonical_source_symbol(symbol)?;
                let data = hir.symbols.get(symbol)?;
                match &data.def {
                    SymbolDef::Item { item } => {
                        return match hir.items.get(*item)? {
                            HirItem::Flow(_) | HirItem::Tool(_) => {
                                Some(Target::Unit(EffectUnit::Item(*item)))
                            }
                            HirItem::Type(_) | HirItem::TypeAlias(_) => Some(Target::Constructor),
                            _ => None,
                        };
                    }
                    SymbolDef::EnumVariant { .. }
                    | SymbolDef::ExternalEnumVariant { .. }
                    | SymbolDef::ImportAlias {
                        origin: etas_hir::ImportAliasOrigin::StdEnumVariant { .. },
                        ..
                    } => return Some(Target::Constructor),
                    SymbolDef::Local { initializer, .. }
                        if matches!(
                            types.facts.symbol_types.get(&symbol),
                            Some(SymbolTypeFact::Local { mutable: false, .. })
                        ) =>
                    {
                        expr = (*initializer)?
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    None
}

fn storage_iterable(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Array(_)
            | Type::List(_)
            | Type::Slice(_)
            | Type::Map { .. }
            | Type::Set(_)
            | Type::Range { .. }
            | Type::Primitive(etas_types::PrimitiveType::Bytes | etas_types::PrimitiveType::String)
    )
}
