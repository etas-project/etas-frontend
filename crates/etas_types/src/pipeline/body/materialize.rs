use etas_core::{Diagnostic, TypeDiagnosticCode};

use super::state::BodyPipelineState;

// Type interning assigns arena IDs: visit checked facts in source-ID order so
// equivalent compilation sessions produce the same portable type graph.
fn ordered_values_mut<K: Ord, V>(
    map: &mut std::collections::HashMap<K, V>,
) -> impl Iterator<Item = &mut V> {
    let mut entries = map.iter_mut().collect::<Vec<_>>();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    entries.into_iter().map(|(_, value)| value)
}

pub fn run(ctx: &mut crate::pipeline::context::TypePipelineContext<'_>, state: BodyPipelineState) {
    let mut state = state;
    if let Err(error) = apply_solver_substitutions(ctx, &mut state) {
        let span = ctx
            .hir
            .items
            .get(state.item)
            .map(etas_hir::HirItem::span)
            .expect("body pipeline item must exist in checked HIR");
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            format!("type substitution failed: {error}"),
        ));
        return;
    }
    if let Err(error) = materialize_generic_type_instances(ctx, &mut state) {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            ctx.hir.items[state.item].span(),
            format!("generic call result materialization failed: {error}"),
        ));
        return;
    }
    if let Err(error) = materialize_callable_references(ctx, &mut state) {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            ctx.hir.items[state.item].span(),
            format!("callable value instantiation failed: {error}"),
        ));
        return;
    }
    if let Err(error) = methods::materialize(ctx, &mut state) {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            ctx.hir.items[state.item].span(),
            format!("checked method ABI instantiation failed: {error}"),
        ));
        return;
    }
    continuation::record_task_owners(ctx, &mut state);
    continuation::validate_uses(ctx, &state);
    let definition_operations = match reflection::materialize(ctx, &state) {
        Ok(facts) => facts,
        Err(error) => {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.items[state.item].span(),
                error,
            ));
            return;
        }
    };
    let agent_controllers = match agent_controller::materialize(ctx, &state) {
        Ok(facts) => facts,
        Err(error) => {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.items[state.item].span(),
                error,
            ));
            return;
        }
    };
    state
        .provisional
        .expr_types
        .extend(state.solver_report.inferred_expr_types.clone());
    for (expr, pack) in &state.solver_report.existential_packs {
        state.provisional.expr_types.insert(*expr, pack.object_type);
    }
    if let Err(error) = crate::ty::materialize_representations(
        &mut ctx.interner,
        state
            .provisional
            .expr_types
            .values()
            .copied()
            .chain(state.provisional.stmt_types.values().copied())
            .chain(state.provisional.pat_types.values().copied())
            .chain(
                state
                    .provisional
                    .field_projections
                    .values()
                    .flatten()
                    .flat_map(|fact| [fact.receiver, fact.output]),
            ),
    ) {
        let span = ctx.hir.items[state.item].span();
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            format!("runtime type representation could not be materialized: {error}"),
        ));
        return;
    }
    ctx.signature_facts
        .definition_operations
        .extend(definition_operations);
    ctx.signature_facts
        .agent_controllers
        .extend(agent_controllers);
    ctx.signature_facts
        .expr_types
        .extend(state.provisional.expr_types);
    ctx.signature_facts
        .agent_method_calls
        .extend(state.provisional.agent_method_calls);
    ctx.signature_facts
        .agent_method_calls
        .extend(state.solver_report.agent_method_calls);
    ctx.signature_facts
        .checked_method_calls
        .extend(state.solver_report.checked_method_calls);
    for expr in state.solver_report.bound_method_values.keys() {
        if let Some(projections) = state.provisional.field_projections.get_mut(expr) {
            projections.pop();
        }
    }
    state
        .provisional
        .field_projections
        .retain(|_, projections| !projections.is_empty());
    ctx.signature_facts
        .bound_method_values
        .extend(state.solver_report.bound_method_values);
    ctx.signature_facts
        .existential_packs
        .extend(state.solver_report.existential_packs);
    ctx.signature_facts
        .reflections
        .extend(state.solver_report.reflections);
    ctx.signature_facts
        .agent_inferences
        .extend(state.solver_report.agent_inferences);
    ctx.signature_facts
        .std_intrinsic_calls
        .extend(state.solver_report.std_intrinsic_calls);
    ctx.signature_facts
        .action_calls
        .extend(state.solver_report.action_calls);
    ctx.signature_facts
        .handler_actions
        .extend(state.provisional.handler_actions);
    ctx.signature_facts
        .continuation_bindings
        .extend(state.provisional.continuation_bindings);
    ctx.signature_facts
        .task_owners
        .extend(state.provisional.task_owners);
    ctx.signature_facts
        .deferred_handler_instantiations
        .extend(state.solver_report.deferred_handler_instantiations);
    ctx.signature_facts
        .expr_memory_places
        .extend(state.provisional.expr_memory_places);
    ctx.signature_facts
        .field_projections
        .extend(state.provisional.field_projections);
    ctx.signature_facts
        .stmt_types
        .extend(state.provisional.stmt_types);
    ctx.signature_facts
        .pattern_types
        .extend(state.provisional.pat_types);
    ctx.signature_facts
        .symbol_types
        .extend(state.provisional.symbol_types);
    ctx.signature_facts
        .try_facts
        .extend(state.provisional.try_facts);
    ctx.signature_facts
        .index_facts
        .extend(state.provisional.index_facts);
    ctx.signature_facts
        .index_facts
        .extend(state.solver_report.index_facts.clone());
    ctx.signature_facts
        .slice_facts
        .extend(state.provisional.slice_facts);
    ctx.signature_facts
        .slice_facts
        .extend(state.solver_report.slice_facts.clone());
    ctx.signature_facts
        .checked_index_errors
        .extend(state.solver_report.checked_index_errors.clone());
    ctx.signature_facts
        .generic_instantiations
        .extend(state.solver_report.generic_instantiations);
    ctx.signature_facts
        .item_signatures
        .extend(state.provisional.item_signatures);
}

mod agent_controller;
mod continuation;
mod methods;
mod reflection;

fn constraint_generics(
    state: &BodyPipelineState,
) -> std::collections::BTreeMap<etas_hir::HirExprId, Vec<crate::CallableGenericParam>> {
    let mut declarations = std::collections::BTreeMap::<_, Vec<_>>::new();
    for constraint in &state.constraints {
        let (expr, parameters) = match constraint {
            crate::TypeConstraint::Callable {
                call: Some(expr),
                generic_params,
                ..
            }
            | crate::TypeConstraint::CallableReference {
                expr,
                generic_params,
                ..
            } => (*expr, generic_params.clone()),
            crate::TypeConstraint::OverloadedCall {
                expr, candidates, ..
            }
            | crate::TypeConstraint::MethodCall {
                expr, candidates, ..
            } => (
                *expr,
                candidates
                    .iter()
                    .flat_map(|candidate| candidate.signature.generic_params.iter().cloned())
                    .collect(),
            ),
            _ => continue,
        };
        declarations.entry(expr).or_default().extend(parameters);
    }
    declarations
}

fn scoped_type_bindings(
    parameters: &[crate::CallableGenericParam],
    fact: &crate::GenericInstantiationFact,
) -> std::collections::HashMap<crate::TypeId, crate::TypeId> {
    parameters
        .iter()
        .filter_map(|parameter| {
            fact.type_bindings
                .iter()
                .find(|(name, _)| name == &parameter.name)
                .map(|(_, ty)| (parameter.subject, *ty))
        })
        .collect()
}

fn materialize_generic_type_instances(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), crate::TypeSubstitutionError> {
    let declarations = constraint_generics(state);
    let mut instantiations = state
        .solver_report
        .generic_instantiations
        .iter()
        .map(|(expr, fact)| (*expr, fact.clone()))
        .collect::<Vec<_>>();
    instantiations.sort_unstable_by_key(|(expr, _)| *expr);
    let callees = state
        .constraints
        .iter()
        .filter_map(|constraint| match constraint {
            crate::TypeConstraint::Callable {
                call: Some(expr),
                callee,
                ..
            } => Some((*expr, *callee)),
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut replacements = std::collections::HashMap::new();
    for (expr, fact) in instantiations {
        let source = state
            .provisional
            .expr_types
            .get(&expr)
            .or_else(|| state.solver_report.inferred_expr_types.get(&expr))
            .copied()
            .or_else(|| {
                state
                    .solver_report
                    .std_intrinsic_calls
                    .get(&expr)
                    .map(|call| call.signature.output)
            });
        let Some(source) = source else {
            continue;
        };
        let parameters =
            scoped_type_bindings(declarations.get(&expr).map_or(&[], Vec::as_slice), &fact);
        let types = std::collections::HashMap::new();
        let source_parameters = std::collections::HashMap::new();
        let rows = fact.effect_row_bindings.into_iter().collect();
        let mut engine = crate::TypeSubstitutionEngine::materializing(
            &mut ctx.interner,
            &types,
            Some(&state.solver_report.substitutions),
        )
        // The result already contains caller-owned type parameters. Substitute
        // the checked callee's subjects, never a caller's reused display name.
        .with_type_parameters(&parameters)
        .with_source_parameters(&source_parameters)
        .with_effect_arguments(&rows);
        // Effects consumes the complete request ABI through a read-only type
        // store, including payloads whose inferred rows differ from the template.
        if let Some(callee) = callees.get(&expr) {
            engine.substitute(*callee)?;
        }
        let materialized = engine.substitute(source)?;
        if materialized != source {
            replacements.insert(source, materialized);
        }
    }
    if replacements.is_empty() {
        return Ok(());
    }

    let names = std::collections::HashMap::new();
    let remap = |ctx: &mut crate::pipeline::context::TypePipelineContext<'_>, ty| {
        crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &names, None)
            .with_type_parameters(&replacements)
            .substitute(ty)
    };
    for ty in ordered_values_mut(&mut state.provisional.expr_types)
        .chain(ordered_values_mut(
            &mut state.solver_report.inferred_expr_types,
        ))
        .chain(ordered_values_mut(&mut state.provisional.stmt_types))
        .chain(ordered_values_mut(&mut state.provisional.pat_types))
    {
        *ty = remap(ctx, *ty)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.symbol_types) {
        match fact {
            crate::SymbolTypeFact::Param { ty }
            | crate::SymbolTypeFact::Local { ty, .. }
            | crate::SymbolTypeFact::Field { ty }
            | crate::SymbolTypeFact::Value { ty }
            | crate::SymbolTypeFact::TopLevelLet { ty, .. } => *ty = remap(ctx, *ty)?,
            _ => {}
        }
    }
    for fact in ordered_values_mut(&mut state.solver_report.std_intrinsic_calls) {
        fact.callable_type = remap(ctx, fact.callable_type)?;
        for ty in fact
            .signature
            .params
            .iter_mut()
            .chain(std::iter::once(&mut fact.signature.output))
        {
            *ty = remap(ctx, *ty)?;
        }
    }
    for fact in ordered_values_mut(&mut state.provisional.handler_actions) {
        fact.output = remap(ctx, fact.output)?;
        for ty in &mut fact.params {
            *ty = remap(ctx, *ty)?;
        }
    }
    for fact in ordered_values_mut(&mut state.provisional.continuation_bindings) {
        fact.action_output = remap(ctx, fact.action_output)?;
        fact.answer = fact.answer.map(|answer| remap(ctx, answer)).transpose()?;
        fact.residual_effects = fact
            .residual_effects
            .take()
            .map(|row| {
                crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &names, None)
                    .with_type_parameters(&replacements)
                    .substitute_effect_row(row)
            })
            .transpose()?;
    }
    Ok(())
}

fn materialize_callable_references(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), String> {
    let declarations = constraint_generics(state);
    for constraint in &state.constraints {
        let crate::TypeConstraint::CallableReference { expr, .. } = constraint else {
            continue;
        };
        let Some(fact) = state.solver_report.generic_instantiations.get(expr) else {
            if !state.solver_report.failures.is_empty() {
                continue;
            }
            return Err(format!(
                "callable reference {expr:?} has no solved instantiation"
            ));
        };
        let ty = state
            .provisional
            .expr_types
            .get_mut(expr)
            .ok_or_else(|| format!("callable reference {expr:?} has no provisional type"))?;
        let parameters =
            scoped_type_bindings(declarations.get(expr).map_or(&[], Vec::as_slice), fact);
        let types = std::collections::HashMap::new();
        let source_parameters = std::collections::HashMap::new();
        let rows = fact.effect_row_bindings.iter().cloned().collect();
        *ty = crate::TypeSubstitutionEngine::materializing(
            &mut ctx.interner,
            &types,
            Some(&state.solver_report.substitutions),
        )
        .with_type_parameters(&parameters)
        .with_source_parameters(&source_parameters)
        .with_effect_arguments(&rows)
        .substitute(*ty)
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn apply_solver_substitutions(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), crate::TypeSubstitutionError> {
    let substitutions = state.solver_report.substitutions.clone();
    let named_substitutions = state.solver_report.named_substitutions.clone();
    if substitutions.iter().next().is_none() && named_substitutions.is_empty() {
        return Ok(());
    }

    for fact in ordered_values_mut(&mut state.solver_report.std_intrinsic_calls) {
        fact.callable_type = substitute_type(
            ctx,
            &substitutions,
            &named_substitutions,
            fact.callable_type,
        )?;
        substitute_callable_signature(
            ctx,
            &substitutions,
            &named_substitutions,
            &mut fact.signature,
        )?;
    }
    for fact in ordered_values_mut(&mut state.solver_report.action_calls) {
        substitute_callable_signature(
            ctx,
            &substitutions,
            &named_substitutions,
            &mut fact.signature,
        )?;
    }

    for fact in ordered_values_mut(&mut state.solver_report.reflections) {
        fact.descriptor_type = substitute_type(
            ctx,
            &substitutions,
            &named_substitutions,
            fact.descriptor_type,
        )?;
        if let crate::CheckedReflectionTarget::CallableValue { signature, .. } = &mut fact.target {
            *signature = substitute_type(ctx, &substitutions, &named_substitutions, *signature)?;
        }
    }

    for ty in ordered_values_mut(&mut state.provisional.expr_types) {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for ty in ordered_values_mut(&mut state.provisional.expr_memory_places) {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.field_projections).flatten() {
        fact.receiver = substitute_type(ctx, &substitutions, &named_substitutions, fact.receiver)?;
        fact.output = substitute_type(ctx, &substitutions, &named_substitutions, fact.output)?;
    }
    for ty in ordered_values_mut(&mut state.provisional.stmt_types) {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for ty in ordered_values_mut(&mut state.provisional.pat_types) {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.symbol_types) {
        substitute_symbol_fact(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.item_signatures) {
        substitute_item_signature(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.try_facts) {
        fact.value_type =
            substitute_type(ctx, &substitutions, &named_substitutions, fact.value_type)?;
        fact.result_type =
            substitute_type(ctx, &substitutions, &named_substitutions, fact.result_type)?;
        if let Some(error) = &mut fact.target_error {
            *error = substitute_type(ctx, &substitutions, &named_substitutions, *error)?;
        }
    }
    for fact in ordered_values_mut(&mut state.provisional.index_facts)
        .chain(ordered_values_mut(&mut state.solver_report.index_facts))
    {
        substitute_index_fact(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.slice_facts)
        .chain(ordered_values_mut(&mut state.solver_report.slice_facts))
    {
        substitute_slice_fact(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for error in ordered_values_mut(&mut state.solver_report.checked_index_errors) {
        *error = substitute_type(ctx, &substitutions, &named_substitutions, *error)?;
    }
    for fact in ordered_values_mut(&mut state.solver_report.agent_inferences) {
        fact.output = substitute_type(ctx, &substitutions, &named_substitutions, fact.output)?;
        fact.schema = substitute_type(ctx, &substitutions, &named_substitutions, fact.schema)?;
    }
    for fact in ordered_values_mut(&mut state.provisional.handler_actions) {
        fact.output = substitute_type(ctx, &substitutions, &named_substitutions, fact.output)?;
        for ty in &mut fact.params {
            *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
        }
        for arg in &mut fact.selectors {
            if let crate::EffectArgRef::Type(ty) = arg {
                *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
            }
        }
    }
    for fact in ordered_values_mut(&mut state.provisional.continuation_bindings) {
        fact.action_output = substitute_type(
            ctx,
            &substitutions,
            &named_substitutions,
            fact.action_output,
        )?;
        fact.answer = fact
            .answer
            .map(|answer| substitute_type(ctx, &substitutions, &named_substitutions, answer))
            .transpose()?;
        fact.residual_effects = fact
            .residual_effects
            .take()
            .map(|row| {
                crate::substitute_effect_row_params(
                    &mut ctx.interner,
                    row,
                    &named_substitutions,
                    &substitutions,
                )
            })
            .transpose()?;
    }
    for fact in ordered_values_mut(&mut state.solver_report.deferred_handler_instantiations) {
        if let Some(overload) = &mut fact.overload {
            for ty in overload
                .params
                .iter_mut()
                .chain(std::iter::once(&mut overload.output))
            {
                *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
            }
            for signature in &mut overload.candidates {
                for ty in signature
                    .params
                    .iter_mut()
                    .chain(std::iter::once(&mut signature.output))
                {
                    *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
                }
            }
        }
        for parameter in &mut fact.parameters {
            parameter.candidate = substitute_type(
                ctx,
                &substitutions,
                &named_substitutions,
                parameter.candidate,
            )?;
            parameter.rigid =
                substitute_type(ctx, &substitutions, &named_substitutions, parameter.rigid)?;
        }
    }
    for fact in ordered_values_mut(&mut state.solver_report.generic_instantiations) {
        for (_, ty) in &mut fact.type_bindings {
            *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
        }
        for (_, row) in &mut fact.effect_row_bindings {
            *row = crate::substitute_effect_row_params(
                &mut ctx.interner,
                row.clone(),
                &named_substitutions,
                &substitutions,
            )?;
        }
        for obligation in &mut fact.deferred_effect_row_obligations {
            obligation.fixed = crate::substitute_effect_row_params(
                &mut ctx.interner,
                obligation.fixed.clone(),
                &named_substitutions,
                &substitutions,
            )?;
            for equation in &mut obligation.concrete {
                equation.fixed = crate::substitute_effect_row_params(
                    &mut ctx.interner,
                    equation.fixed.clone(),
                    &named_substitutions,
                    &substitutions,
                )?;
                equation.actual = crate::substitute_effect_row_params(
                    &mut ctx.interner,
                    equation.actual.clone(),
                    &named_substitutions,
                    &substitutions,
                )?;
            }
        }
    }
    for fact in ordered_values_mut(&mut state.solver_report.checked_method_calls).chain(
        ordered_values_mut(&mut state.solver_report.bound_method_values)
            .map(|value| &mut value.method),
    ) {
        fact.try_map_types(|ty| substitute_type(ctx, &substitutions, &named_substitutions, ty))?;
    }
    for fact in ordered_values_mut(&mut state.solver_report.existential_packs) {
        fact.try_map_types(|ty| substitute_type(ctx, &substitutions, &named_substitutions, ty))?;
    }
    Ok(())
}

fn substitute_symbol_fact(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    fact: &mut crate::SymbolTypeFact,
) -> Result<(), crate::TypeSubstitutionError> {
    match fact {
        crate::SymbolTypeFact::Param { ty }
        | crate::SymbolTypeFact::Local { ty, .. }
        | crate::SymbolTypeFact::Field { ty }
        | crate::SymbolTypeFact::Value { ty }
        | crate::SymbolTypeFact::TopLevelLet { ty, .. } => {
            *ty = substitute_type(ctx, substitutions, named, *ty)?;
        }
        crate::SymbolTypeFact::Flow { signature } | crate::SymbolTypeFact::Tool { signature } => {
            substitute_callable_signature(ctx, substitutions, named, signature)?;
        }
        crate::SymbolTypeFact::Agent { signature } => {
            substitute_agent_signature(ctx, substitutions, named, signature)?
        }
        _ => {}
    }
    Ok(())
}

fn substitute_item_signature(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    signature: &mut crate::ItemSignature,
) -> Result<(), crate::TypeSubstitutionError> {
    match signature {
        crate::ItemSignature::Flow(signature) | crate::ItemSignature::Tool(signature) => {
            substitute_callable_signature(ctx, substitutions, named, signature)?;
        }
        crate::ItemSignature::Agent(signature) => {
            substitute_agent_signature(ctx, substitutions, named, signature)?
        }
        crate::ItemSignature::TopLevelLet(signature) => {
            signature.ty = substitute_type(ctx, substitutions, named, signature.ty)?;
        }
    }
    Ok(())
}

fn substitute_agent_signature(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    signature: &mut crate::AgentSignature,
) -> Result<(), crate::TypeSubstitutionError> {
    signature.component = substitute_type(ctx, substitutions, named, signature.component)?;
    signature.definition_type = signature
        .definition_type
        .map(|ty| substitute_type(ctx, substitutions, named, ty))
        .transpose()?;
    if let Some(entry) = &mut signature.default_entry {
        let entry = std::sync::Arc::make_mut(entry);
        entry.callable_type = substitute_type(ctx, substitutions, named, entry.callable_type)?;
        substitute_callable_signature(ctx, substitutions, named, &mut entry.callable)?;
    }
    Ok(())
}

fn substitute_callable_signature(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    signature: &mut crate::CallableSignature,
) -> Result<(), crate::TypeSubstitutionError> {
    for param in &mut signature.params {
        *param = substitute_type(ctx, substitutions, named, *param)?;
    }
    signature.output = substitute_type(ctx, substitutions, named, signature.output)?;
    for param in &mut signature.generic_params {
        param.subject = substitute_type(ctx, substitutions, named, param.subject)?;
        for bound in &mut param.bounds {
            for arg in &mut bound.args {
                *arg = substitute_type(ctx, substitutions, named, *arg)?;
            }
        }
    }
    if let Some(row) = &mut signature.effects {
        *row = crate::substitute_effect_row_params(
            &mut ctx.interner,
            row.clone(),
            named,
            substitutions,
        )?;
    }
    if let Some(row) = &mut signature.requested_actions {
        *row = crate::substitute_effect_row_params(
            &mut ctx.interner,
            row.clone(),
            named,
            substitutions,
        )?;
    }
    Ok(())
}

fn substitute_index_fact(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    fact: &mut crate::CheckedIndexKind,
) -> Result<(), crate::TypeSubstitutionError> {
    match fact {
        crate::CheckedIndexKind::Sequence {
            base,
            index,
            output,
        } => {
            *base = substitute_type(ctx, substitutions, named, *base)?;
            *index = substitute_type(ctx, substitutions, named, *index)?;
            *output = substitute_type(ctx, substitutions, named, *output)?;
        }
        crate::CheckedIndexKind::MapLookup { key, value } => {
            *key = substitute_type(ctx, substitutions, named, *key)?;
            *value = substitute_type(ctx, substitutions, named, *value)?;
        }
    }
    Ok(())
}

fn substitute_slice_fact(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    fact: &mut crate::CheckedSliceKind,
) -> Result<(), crate::TypeSubstitutionError> {
    let (base, start, end, output) = match fact {
        crate::CheckedSliceKind::Sequence {
            base,
            start,
            end,
            output,
        }
        | crate::CheckedSliceKind::Range {
            range: base,
            start,
            end,
            output,
        } => (base, start, end, output),
    };
    *base = substitute_type(ctx, substitutions, named, *base)?;
    *start = substitute_type(ctx, substitutions, named, *start)?;
    *end = substitute_type(ctx, substitutions, named, *end)?;
    *output = substitute_type(ctx, substitutions, named, *output)?;
    Ok(())
}

fn substitute_type(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named: &std::collections::HashMap<String, crate::TypeId>,
    ty: crate::TypeId,
) -> Result<crate::TypeId, crate::TypeSubstitutionError> {
    crate::substitute_type_params(&mut ctx.interner, ty, named, substitutions)
}
