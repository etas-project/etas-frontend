use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_types::Type;

use crate::{
    Effect, EffectAnalysisOutput, EffectFacts, EffectOutput, EffectPipelineError, EffectRegistry,
    EffectRow, EffectSet, EffectUnit, LatentEffectBodyRef, LatentEffectFact,
    effect_var_id_from_name,
};

mod contextual_rows;

pub struct EffectFactProjector;

impl EffectFactProjector {
    pub fn materialize(
        hir: &etas_hir::HirProgram,
        types: &etas_types::TypeOutput,
        registry: &EffectRegistry,
        mut analysis: EffectAnalysisOutput,
        reachable_items: Option<&std::collections::BTreeSet<HirItemId>>,
    ) -> Result<EffectOutput, EffectPipelineError> {
        let mut latent_realizations = std::mem::take(&mut analysis.inputs.latent_realizations);
        let unit_effects = analysis
            .summaries
            .iter()
            .map(|(unit, summary)| (*unit, summary.clone()))
            .chain(analysis.inputs.unit_effects)
            .collect();
        let mut facts = EffectFacts {
            external_descriptor_graphs: analysis.inputs.external_descriptor_graphs,
            handled_body_requests: analysis.inputs.handled_body_requests,
            frozen_callable_graphs: analysis.inputs.frozen_callable_graphs,
            callable_references: analysis.inputs.callable_references,
            metadata_callables: analysis.inputs.metadata_callables,
            metadata_callable_graphs: analysis.inputs.metadata_callable_graphs,
            captured_callables: analysis.inputs.captured_callables,
            native_call_targets: analysis.inputs.native_call_targets,
            metadata_call_targets: analysis.inputs.metadata_call_targets,
            generic_effect_rows: analysis.inputs.generic_effect_rows,
            callable_environments: analysis.inputs.callable_environments,
            method_evidence: analysis.inputs.method_evidence,
            inferred_parameter_effects: analysis.inputs.inferred_parameter_effects,
            handler_action_effects: analysis.inputs.handler_action_effects,
            expr_effects: analysis.inputs.expr_effects,
            stmt_effects: analysis.inputs.stmt_effects,
            unit_effects,
            handler_values: analysis.inputs.handler_values,
            handle_applications: analysis.inputs.handle_applications,
            performed_actions: analysis.inputs.performed_actions,
            try_captures: analysis.inputs.try_captures,
            latent_effects: analysis.inputs.latent_effects,
            std_callable_values: analysis.inputs.std_callable_values,
            std_action_calls: analysis.inputs.std_action_calls,
            ..EffectFacts::default()
        };
        let mut diagnostics = analysis.diagnostics;

        materialize_latent_effects(
            hir,
            types,
            registry,
            &mut facts,
            &mut diagnostics,
            &mut latent_realizations,
        )?;
        materialize_task_calls(hir, types, registry, &mut facts, &mut diagnostics)?;

        for (item, hir_item) in hir.items.iter() {
            if reachable_items.is_some_and(|reachable| !reachable.contains(&item)) {
                continue;
            }
            if !requires_materialized_item_summary(hir_item) {
                continue;
            }
            let summary_item = if let HirItem::Agent(agent) = hir_item
                && !matches!(agent.body, etas_hir::HirAgentBody::Source { .. })
            {
                let Some(etas_types::ItemSignature::Agent(signature)) =
                    types.facts.item_signatures.get(&item)
                else {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        agent.span,
                        "agent component is missing checked signature facts",
                    ));
                    continue;
                };
                if signature.default_entry.is_none() {
                    continue;
                }
                let Some(method) = types.facts.agent_default_method(item) else {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        agent.span,
                        "agent default invocation is missing checked method identity",
                    ));
                    continue;
                };
                method.body
            } else {
                item
            };
            let Some(summary) = facts
                .unit_effects
                .get(&EffectUnit::Item(summary_item))
                .cloned()
            else {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    hir_item.span(),
                    "effect fact materialization requires a solved item summary",
                ));
                continue;
            };
            let Some(symbol) = item_symbol(hir_item) else {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    hir_item.span(),
                    "effect fact materialization requires a materialized item symbol",
                ));
                continue;
            };
            facts.item_effects.insert(item, summary.clone());
            facts.symbol_effects.insert(symbol, summary.clone());
            facts
                .requirements
                .items
                .insert(item, summary.requirements.clone());
            facts
                .requirements
                .symbols
                .insert(symbol, summary.requirements.clone());
            facts
                .interpreter_support
                .items
                .insert(item, summary.support.clone());
        }

        Ok(EffectOutput { facts, diagnostics })
    }
}

fn materialize_task_calls(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    facts: &mut EffectFacts,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), EffectPipelineError> {
    use etas_hir::{HirExpr, HirNodeRef, ResolveResult};

    let view = etas_hir::HirTreeView::new(hir);
    let run_callbacks = types
        .facts
        .std_intrinsic_calls
        .iter()
        .filter(|(_, fact)| fact.intrinsic == etas_std::intrinsic::task::RUN)
        .filter_map(|(call, _)| argument(hir, *call, 0))
        .collect::<std::collections::HashSet<_>>();

    for (call, intrinsic) in &types.facts.std_intrinsic_calls {
        let Some(kind) = task_call_kind(intrinsic.intrinsic) else {
            continue;
        };
        let span = hir
            .exprs
            .get(*call)
            .map(|expr| expr.span(&hir.blocks))
            .ok_or(EffectPipelineError::MissingExprFact { expr: *call })?;
        let Some(output) = types.facts.expr_types.get(call).copied() else {
            diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "structured task call has no checked output type",
            ));
            continue;
        };
        let Some(behavior) = facts.expr_effects.get(call).cloned() else {
            diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "structured task call has no checked effect summary",
            ));
            continue;
        };
        let job = match kind {
            crate::TaskCallKind::Run | crate::TaskCallKind::Start => argument(hir, *call, 0),
            crate::TaskCallKind::MapConcurrent => call_arguments(hir, *call).last().copied(),
            _ => None,
        };
        let owner_scope = match kind {
            crate::TaskCallKind::Run => job
                .and_then(|job| match hir.exprs.get(job) {
                    Some(HirExpr::Lambda { scope, .. }) => Some(*scope),
                    _ => None,
                })
                .or_else(|| view.enclosing_scope(HirNodeRef::Expr(*call))),
            crate::TaskCallKind::Start => types
                .facts
                .task_owners
                .get(call)
                .map(|owner| owner.owner_scope)
                .or_else(|| task_owner_scope(view.index(), hir, *call, &run_callbacks)),
            _ => task_owner_scope(view.index(), hir, *call, &run_callbacks),
        };
        let Some(owner_scope) = owner_scope else {
            diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "structured task call has no checked scheduler owner scope",
            ));
            continue;
        };
        let effects = task_call_effects(hir, types, registry, facts, *call, kind);
        facts.task_calls.insert(
            *call,
            crate::TaskCallFact {
                kind,
                call: *call,
                owner_scope,
                job,
                output,
                effects,
                behavior,
            },
        );

        let continuation_index = match kind {
            crate::TaskCallKind::Requeue => Some(1),
            crate::TaskCallKind::Park => Some(2),
            _ => None,
        };
        let Some(continuation) = continuation_index.and_then(|index| argument(hir, *call, index))
        else {
            continue;
        };
        let Some(HirExpr::Path(path)) = hir.exprs.get(continuation) else {
            continue;
        };
        let ResolveResult::Resolved(binder) = path.resolution else {
            continue;
        };
        if !types.facts.continuation_bindings.contains_key(&binder) {
            continue;
        }
        let target = match kind {
            crate::TaskCallKind::Requeue => crate::TransferTarget::Requeue,
            crate::TaskCallKind::Park => {
                let Some(task) = argument(hir, *call, 1) else {
                    continue;
                };
                crate::TransferTarget::Park { task }
            }
            _ => unreachable!("only continuation transfer calls have a continuation index"),
        };
        facts.continuation_transfers.insert(
            *call,
            crate::ContinuationTransferFact {
                call: *call,
                binder,
                target,
            },
        );
    }
    Ok(())
}

fn task_call_kind(intrinsic: u32) -> Option<crate::TaskCallKind> {
    Some(match intrinsic {
        etas_std::intrinsic::task::RUN => crate::TaskCallKind::Run,
        etas_std::intrinsic::task::START => crate::TaskCallKind::Start,
        etas_std::intrinsic::task::AWAIT => crate::TaskCallKind::Await,
        etas_std::intrinsic::task::REQUEUE => crate::TaskCallKind::Requeue,
        etas_std::intrinsic::task::PARK => crate::TaskCallKind::Park,
        etas_std::intrinsic::task::JOIN_TUPLE | etas_std::intrinsic::task::JOIN_ARRAY => {
            crate::TaskCallKind::Join
        }
        etas_std::intrinsic::task::TRY_JOIN => crate::TaskCallKind::TryJoin,
        etas_std::intrinsic::task::COLLECT => crate::TaskCallKind::Collect,
        etas_std::intrinsic::task::RACE => crate::TaskCallKind::Race,
        etas_std::intrinsic::task::MAP_CONCURRENT
        | etas_std::intrinsic::task::MAP_CONCURRENT_COLLECT => crate::TaskCallKind::MapConcurrent,
        _ => return None,
    })
}

fn task_call_effects(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    facts: &EffectFacts,
    call: etas_hir::HirExprId,
    kind: crate::TaskCallKind,
) -> EffectRow {
    if let Some(row) = facts.generic_effect_rows.get(&call).and_then(|rows| {
        rows.iter()
            .find_map(|(name, row)| (name == "E").then(|| row.clone()))
    }) {
        return row;
    }
    let args = call_arguments(hir, call);
    let selected = match kind {
        crate::TaskCallKind::Run
        | crate::TaskCallKind::Start
        | crate::TaskCallKind::Await
        | crate::TaskCallKind::Join
        | crate::TaskCallKind::TryJoin
        | crate::TaskCallKind::Collect
        | crate::TaskCallKind::Race => args.get(..1).unwrap_or_default(),
        crate::TaskCallKind::Park => args.get(1..2).unwrap_or_default(),
        crate::TaskCallKind::MapConcurrent => args.as_slice(),
        crate::TaskCallKind::Requeue => &[],
    };
    let mut effects = EffectRow::empty();
    let mut visited = std::collections::HashSet::new();
    for argument in selected {
        if let Some(ty) = types.facts.expr_types.get(argument).copied() {
            collect_task_effects(types, registry, ty, &mut effects, &mut visited);
        }
    }
    effects
}

fn collect_task_effects(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    ty: etas_types::TypeId,
    effects: &mut EffectRow,
    visited: &mut std::collections::HashSet<etas_types::TypeId>,
) {
    if !visited.insert(ty) {
        return;
    }
    match types.store.get(ty) {
        Some(etas_types::Type::Task(task)) => {
            effects.union_assign(&row_from_type_ref(registry, &task.effects));
        }
        Some(etas_types::Type::Function(flow)) => {
            if let Some(row) = &flow.effects {
                effects.union_assign(&row_from_type_ref(registry, row));
            }
        }
        Some(etas_types::Type::Tuple(items)) => {
            for item in items {
                collect_task_effects(types, registry, *item, effects, visited);
            }
        }
        Some(
            etas_types::Type::Array(inner)
            | etas_types::Type::List(inner)
            | etas_types::Type::Set(inner)
            | etas_types::Type::Slice(inner)
            | etas_types::Type::Option(inner)
            | etas_types::Type::Schema(inner)
            | etas_types::Type::Message(inner),
        ) => collect_task_effects(types, registry, *inner, effects, visited),
        Some(etas_types::Type::Result { ok, err }) => {
            collect_task_effects(types, registry, *ok, effects, visited);
            collect_task_effects(types, registry, *err, effects, visited);
        }
        Some(etas_types::Type::Record(record)) => {
            for field in &record.fields {
                collect_task_effects(types, registry, field.ty, effects, visited);
            }
        }
        _ => {}
    }
}

fn call_arguments(
    hir: &etas_hir::HirProgram,
    call: etas_hir::HirExprId,
) -> Vec<etas_hir::HirExprId> {
    match hir.exprs.get(call) {
        Some(etas_hir::HirExpr::Call { args, .. }) => {
            args.iter().filter_map(argument_expr).collect()
        }
        Some(etas_hir::HirExpr::MethodCall { receiver, args, .. }) => std::iter::once(*receiver)
            .chain(args.iter().filter_map(argument_expr))
            .collect(),
        _ => vec![],
    }
}

fn argument(
    hir: &etas_hir::HirProgram,
    call: etas_hir::HirExprId,
    index: usize,
) -> Option<etas_hir::HirExprId> {
    call_arguments(hir, call).get(index).copied()
}

fn argument_expr(argument: &etas_hir::HirArg) -> Option<etas_hir::HirExprId> {
    match argument {
        etas_hir::HirArg::Positional(expr) | etas_hir::HirArg::Named { value: expr, .. } => {
            Some(*expr)
        }
    }
}

fn task_owner_scope(
    index: &etas_hir::HirTreeIndex,
    hir: &etas_hir::HirProgram,
    expr: etas_hir::HirExprId,
    run_callbacks: &std::collections::HashSet<etas_hir::HirExprId>,
) -> Option<etas_hir::ScopeId> {
    use etas_hir::{HirBodyKind, HirExpr, HirOwner};

    let mut body = body_containing_expr(index, expr)?;
    loop {
        body = match body.kind {
            HirBodyKind::Lambda(lambda) => {
                if run_callbacks.contains(&lambda) {
                    let HirExpr::Lambda { scope, .. } = hir.exprs[lambda] else {
                        return None;
                    };
                    return Some(scope);
                }
                body_containing_expr(index, lambda)?
            }
            HirBodyKind::HandlerArm(arm) => {
                let HirOwner::Expr(handler) = *index.handler_arm_owner.get(&arm)? else {
                    return None;
                };
                body_containing_expr(index, handler)?
            }
            HirBodyKind::MatchArm { expr, .. } | HirBodyKind::StageLimit { expr, .. } => {
                body_containing_expr(index, expr)?
            }
            HirBodyKind::Flow
            | HirBodyKind::Tool
            | HirBodyKind::Agent
            | HirBodyKind::TopLevelLet
            | HirBodyKind::ImplFlow(_)
            | HirBodyKind::AnnotationArg { .. } => return None,
        };
    }
}

fn body_containing_expr(
    index: &etas_hir::HirTreeIndex,
    expr: etas_hir::HirExprId,
) -> Option<etas_hir::HirBodyRef> {
    use etas_hir::HirOwner;

    let mut owner = *index.expr_owner.get(&expr)?;
    loop {
        owner = match owner {
            HirOwner::Body(body) => return Some(body),
            HirOwner::Expr(expr) => *index.expr_owner.get(&expr)?,
            HirOwner::Block(block) => *index.block_owner.get(&block)?,
            HirOwner::Stmt(stmt) => {
                let block = *index.stmt_block.get(&stmt)?;
                *index.block_owner.get(&block)?
            }
            HirOwner::HandlerArm(_)
            | HirOwner::Item(_)
            | HirOwner::Module(_)
            | HirOwner::Pattern(_) => return None,
        };
    }
}

fn materialize_latent_effects(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    facts: &mut EffectFacts,
    diagnostics: &mut Vec<Diagnostic>,
    latent_realizations: &mut std::collections::HashMap<
        etas_hir::HirExprId,
        Vec<etas_hir::HirExprId>,
    >,
) -> Result<(), EffectPipelineError> {
    let contextual_rows = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
    {
        // Failed analysis retains its diagnostics, but cannot publish checked facts.
        std::collections::HashMap::new()
    } else {
        contextual_rows::solved_contextual_rows(types, registry, facts)?
    };
    for (unit, summary) in facts.unit_effects.clone() {
        let EffectUnit::AnonymousFlow { value, body, .. } = unit else {
            continue;
        };
        let Some(flow_type) = types.facts.expr_types.get(&value).copied() else {
            let span = hir
                .exprs
                .get(value)
                .map(|expr| expr.span(&hir.blocks))
                .ok_or(EffectPipelineError::MissingExprFact { expr: value })?;
            diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "latent flow fact materialization requires a checked function type fact",
            ));
            continue;
        };
        let mut declared_bound = match types.store.get(flow_type) {
            Some(Type::Function(flow)) => flow
                .effects
                .as_ref()
                .map(|row| row_from_type_ref(registry, row)),
            _ => {
                let span = hir
                    .exprs
                    .get(value)
                    .map(|expr| expr.span(&hir.blocks))
                    .ok_or(EffectPipelineError::MissingExprFact { expr: value })?;
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "latent flow fact materialization requires a checked function type",
                ));
                continue;
            }
        };
        if let Some(bound) = &mut declared_bound
            && let Some(bindings) = contextual_rows.get(&value)
        {
            contextual_rows::specialize_bound(bound, bindings);
        }
        let body = match body {
            crate::infer::unit::EffectAnonymousFlowBody::Expr(expr) => {
                LatentEffectBodyRef::Expr(expr)
            }
            crate::infer::unit::EffectAnonymousFlowBody::Block(block) => {
                LatentEffectBodyRef::Block(block)
            }
        };
        let realized_at = latent_realizations.remove(&value).unwrap_or_default();
        facts.latent_effects.insert(
            value,
            LatentEffectFact {
                value,
                body,
                flow_type,
                inferred: summary.escaping_effects.clone(),
                declared_bound,
                summary,
                realized_at,
            },
        );
    }
    Ok(())
}

fn row_from_type_ref(registry: &EffectRegistry, row: &etas_types::EffectRowRef) -> EffectRow {
    EffectRow {
        effects: EffectSet::from_iter(
            row.effects
                .iter()
                .filter_map(|effect| effect_from_type_ref(registry, effect)),
        ),
        open: row.tail.as_deref().map(effect_var_id_from_name),
    }
}

fn effect_from_type_ref(
    registry: &EffectRegistry,
    effect: &etas_types::EffectRef,
) -> Option<Effect> {
    if is_core_error_effect_name(registry, &effect.name) {
        if let Some(etas_types::EffectArgRef::Type(error)) = effect.args.first() {
            return Some(Effect::Error(*error));
        }
    }
    if let Some(action) = registry.action_by_name(&effect.name) {
        return if effect.args.is_empty() {
            Some(Effect::Action(action))
        } else {
            Some(Effect::AppliedAction(crate::ActionInstanceRef {
                action,
                args: effect.args.clone(),
            }))
        };
    }
    registry.tag_by_name(&effect.name).map(|tag| {
        if effect.args.is_empty() {
            Effect::Tag(tag)
        } else {
            Effect::Applied {
                tag,
                args: effect
                    .args
                    .iter()
                    .filter_map(|arg| match arg {
                        etas_types::EffectArgRef::Type(ty) => Some(*ty),
                        _ => None,
                    })
                    .collect(),
            }
        }
    })
}

fn is_core_error_effect_name(registry: &EffectRegistry, name: &str) -> bool {
    name == "Error"
        || registry.tag_by_name(name) == Some(crate::ERROR_TAG)
        || (name.starts_with("std.") && name.rsplit('.').next() == Some("Error"))
}

fn requires_materialized_item_summary(item: &HirItem) -> bool {
    matches!(
        item,
        HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_) | HirItem::TopLevelLet(_)
    )
}

fn item_symbol(item: &HirItem) -> Option<SymbolId> {
    match item {
        HirItem::Flow(flow) => Some(flow.symbol),
        HirItem::Tool(tool) => Some(tool.symbol),
        HirItem::Agent(agent) => Some(agent.symbol),
        HirItem::TopLevelLet(value) => Some(value.symbol),
        _ => None,
    }
}
