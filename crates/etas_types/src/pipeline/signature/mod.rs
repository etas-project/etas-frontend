pub mod collect {
    pub mod agent_methods;
    pub mod aliases;
    pub mod callables;
    pub mod effect_actions;
    pub mod imports;
    pub mod methods;
    pub mod nominal_types;
    pub mod resource_handles;
    pub mod specs;
    pub mod std_actions;
    pub mod trace_patterns;
    pub mod type_impl_methods;
}
pub mod imports;
pub mod materialize;
pub mod solve {
    pub mod aliases;
    pub mod spec_closure;
}
pub mod state;
pub mod validate {
    pub mod annotations;
    pub mod exported_contracts;
    pub mod flow_spec_satisfaction;
    pub mod spec_impls;
    pub(super) mod spec_methods;
    pub(super) mod type_applications;
}

use crate::pipeline::context::TypePipelineContext;

fn capability_field(
    ctx: &TypePipelineContext<'_>,
    owner: Option<crate::TypeId>,
    field: &str,
) -> Option<crate::TypeId> {
    let crate::Type::Nominal(owner) = ctx.interner.store().get(owner?)? else {
        return None;
    };
    let crate::Type::Record(record) = ctx.interner.store().get(owner.representation?)? else {
        return None;
    };
    record
        .fields
        .iter()
        .find(|slot| slot.name == field)
        .map(|slot| slot.ty)
}

pub fn run_for_hir(ctx: &mut TypePipelineContext<'_>) {
    imports::std::apply_hir_std_prelude(ctx);
    run(ctx);
}

pub fn run_from_input<'a>(
    ctx: &mut TypePipelineContext<'a>,
    input: &crate::SignaturePipelineInput<'a>,
) {
    ctx.hir_view = input.view.clone();
    if let Some(prepared) = &input.prepared_external {
        prepared.initialize(ctx, &input.external);
    }
    imports::reflection::initialize(ctx, &input.reflection);
    imports::std::apply_hir_std_prelude(ctx);
    imports::std::apply_std_signature_input(ctx, &input.std);
    if let Some(prepared) = &input.prepared_external {
        let mut other = input.external.clone();
        other
            .metadata
            .retain(|metadata| metadata.package != prepared.package);
        other
            .symbol_bindings
            .retain(|binding| binding.package != prepared.package);
        other
            .action_bindings
            .retain(|binding| binding.package != prepared.package);
        other
            .source_spec_bindings
            .retain(|binding| binding.package != prepared.package);
        imports::external::apply_external_signature_input(ctx, &other);
        prepared.bind(ctx, &input.external);
    } else {
        imports::external::apply_external_signature_input(ctx, &input.external);
    }
    if let Some(package) = input.external.standard_package {
        imports::prepared::bind_standard_identities(ctx, package);
    }
    run_with_annotations(ctx, input.annotation_validation);
    imports::source::apply_source_signature_input(ctx, &input.source);
    crate::task_scope::infer(ctx);
    imports::reflection::apply(ctx, &input.reflection);
}

pub fn run(ctx: &mut TypePipelineContext<'_>) {
    run_with_annotations(ctx, crate::AnnotationValidationMode::Immediate);
    crate::task_scope::infer(ctx);
}

fn run_with_annotations(ctx: &mut TypePipelineContext<'_>, mode: crate::AnnotationValidationMode) {
    let mut state = state::SignaturePipelineState::default();
    collect::imports::collect_imports(ctx, &mut state);
    collect::nominal_types::collect_nominal_types(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::aliases::collect_aliases(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::specs::record_type_param_bounds(ctx, &mut state);
    collect::specs::collect_specs(ctx, &mut state);
    collect::aliases::collect_alias_bounds(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::trace_patterns::collect_trace_pattern_types(ctx);
    collect::nominal_types::collect_members(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    if ctx.hir.source_standard_types {
        ctx.signature_facts.known_std_types.filesystem_io_error =
            crate::lower::type_ref::selected_source_type(ctx, &["std", "io", "IOError"]);
        ctx.signature_facts.known_std_types.option_enum =
            crate::lower::type_ref::selected_source_type(ctx, &["std", "option", "Option"]);
        ctx.signature_facts.known_std_types.result_enum =
            crate::lower::type_ref::selected_source_type(ctx, &["std", "result", "Result"]);
        ctx.signature_facts.known_std_types.source_iterator =
            crate::lower::type_ref::selected_source_type(ctx, &["std", "collections", "Iter"]);
        ctx.signature_facts.known_std_types.source_slice =
            crate::lower::type_ref::selected_source_type(ctx, &["std", "collections", "Slice"]);
        for (name, wrapper) in [
            ("Trusted", crate::TrustWrapper::Trusted),
            ("Untrusted", crate::TrustWrapper::Untrusted),
            ("Public", crate::TrustWrapper::Public),
            ("Sanitized", crate::TrustWrapper::Sanitized),
            ("Secret", crate::TrustWrapper::Secret),
        ] {
            if let Some(ty) = crate::lower::type_ref::selected_source_type(
                ctx,
                &["std", "security", "trust", name],
            ) {
                ctx.signature_facts
                    .known_std_types
                    .trust_wrappers
                    .insert(ty, wrapper);
            }
        }
        ctx.signature_facts.known_std_types.index_error =
            crate::lower::type_ref::selected_source_type(
                ctx,
                &["std", "runtime", "error", "IndexError"],
            );
        ctx.signature_facts.known_std_types.budget_exceeded =
            crate::lower::type_ref::selected_source_type(
                ctx,
                &["std", "runtime", "error", "BudgetExceeded"],
            );
        let selected = |ctx: &mut TypePipelineContext<'_>, name: &str| {
            crate::lower::type_ref::selected_source_type(ctx, &["std", "task", name])
        };
        for name in [
            "Store",
            "MemoryRegion",
            "MemorySelection",
            "MemoryPlace",
            "MemoryTransaction",
        ] {
            if let Some(ty) =
                crate::lower::type_ref::selected_source_type(ctx, &["std", "memory", name])
            {
                ctx.signature_facts
                    .known_std_types
                    .storage_resources
                    .insert(ty);
            }
        }
        let task = selected(ctx, "Task");
        let scheduler = selected(ctx, "Scheduler");
        let continuation = selected(ctx, "Continuation");
        let instant = crate::lower::type_ref::selected_source_type(
            ctx,
            &["std", "runtime", "time", "Instant"],
        );
        let clock_domain = capability_field(ctx, instant, "clock");
        let read_owner =
            crate::lower::type_ref::selected_source_type(ctx, &["std", "stream", "ReadOwner"]);
        ctx.signature_facts.known_std_types.execution =
            crate::facts::symbols::KnownExecutionTypes {
                task,
                scheduler,
                continuation,
                task_handle: capability_field(ctx, task, "handle"),
                scheduler_state: capability_field(ctx, scheduler, "state"),
                continuation_state: capability_field(ctx, continuation, "state"),
            };
        let session_lease = crate::lower::type_ref::selected_source_type(
            ctx,
            &["std", "agent", "session", "SessionLease"],
        );
        let known = &mut ctx.signature_facts.known_std_types;
        known.opaque_values.extend(
            [
                known.execution.task_handle,
                known.execution.scheduler_state,
                known.execution.continuation_state,
                clock_domain,
                read_owner,
                session_lease,
            ]
            .into_iter()
            .flatten(),
        );
    }
    collect::effect_actions::collect_effect_actions(ctx, &mut state);
    collect::std_actions::collect_std_actions(ctx, &mut state);
    publish_action_signature_prefix(ctx, &state);
    collect::callables::collect_callables(ctx, &mut state);
    collect::type_impl_methods::collect_type_impl_methods(ctx, &mut state);
    collect::agent_methods::collect_agent_methods(ctx, &mut state);
    collect::agent_methods::collect_agent_spec_satisfactions(ctx, &mut state);
    solve::aliases::solve_aliases(ctx, &mut state);
    solve::spec_closure::solve_spec_closure(ctx, &mut state);
    validate::spec_impls::validate_spec_impls(ctx, &state);
    validate::flow_spec_satisfaction::validate_flow_spec_satisfaction(ctx, &mut state);
    validate::exported_contracts::validate_exported_contracts(ctx, &state);
    collect::methods::collect(ctx, &mut state);
    materialize::materialize_signature_facts(ctx, state);
    validate::type_applications::run(ctx);
    let injection_owners = ctx
        .hir
        .items
        .iter()
        .filter_map(|(item, _)| {
            crate::pipeline::body::collect::injections::fields(ctx.hir, item)
                .iter()
                .any(|field| !field.annotations.is_empty())
                .then_some(item)
        })
        .collect::<Vec<_>>();
    for owner in injection_owners {
        crate::pipeline::body::run(ctx, owner);
    }
    if mode == crate::AnnotationValidationMode::Immediate {
        ctx.diagnostics
            .extend(validate::annotations::validate_annotations(
                ctx.hir,
                &ctx.signature_facts,
                &ctx.std_registry,
            ));
    }
}

fn publish_action_signature_prefix(
    ctx: &mut TypePipelineContext<'_>,
    state: &state::SignaturePipelineState,
) {
    ctx.signature_facts.action_signatures.extend(
        state
            .action_signatures
            .iter()
            .map(|(symbol, signature)| (*symbol, signature.clone())),
    );
    ctx.signature_facts.qualified_action_signatures.extend(
        state
            .qualified_action_signatures
            .iter()
            .map(|(name, signature)| (name.clone(), signature.clone())),
    );
    ctx.signature_facts
        .symbol_types
        .extend(
            state
                .symbol_types
                .iter()
                .filter_map(|(symbol, fact)| match fact {
                    crate::SymbolTypeFact::EffectAction { .. } => Some((*symbol, fact.clone())),
                    _ => None,
                }),
        );
}

fn publish_signature_prefix(
    ctx: &mut TypePipelineContext<'_>,
    state: &state::SignaturePipelineState,
) {
    ctx.signature_facts.symbol_types.extend(
        state
            .symbol_types
            .iter()
            .map(|(symbol, fact)| (*symbol, fact.clone())),
    );
    ctx.signature_facts.type_param_bounds.extend(
        state
            .type_param_bounds
            .iter()
            .map(|(symbol, facts)| (*symbol, facts.clone())),
    );
    ctx.signature_facts.spec_signatures.extend(
        state
            .spec_signatures
            .iter()
            .map(|(symbol, fact)| (*symbol, fact.clone())),
    );
    ctx.signature_facts.enum_layouts.extend(
        state
            .enum_layouts
            .iter()
            .map(|(ty, layout)| (*ty, layout.clone())),
    );
}
