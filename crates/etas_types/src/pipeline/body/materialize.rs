use etas_core::{Diagnostic, TypeDiagnosticCode};

use super::state::BodyPipelineState;

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
    state
        .provisional
        .expr_types
        .extend(state.solver_report.inferred_expr_types.clone());
    if let Err(error) = crate::ty::materialize_representations(
        &mut ctx.interner,
        state
            .provisional
            .expr_types
            .values()
            .copied()
            .chain(state.provisional.stmt_types.values().copied())
            .chain(state.provisional.pat_types.values().copied()),
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
        .expr_types
        .extend(state.provisional.expr_types);
    ctx.signature_facts
        .expr_memory_places
        .extend(state.provisional.expr_memory_places);
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

fn apply_solver_substitutions(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), crate::TypeSubstitutionError> {
    let substitutions = state.solver_report.substitutions.clone();
    let named_substitutions = state.solver_report.named_substitutions.clone();
    if substitutions.iter().next().is_none() && named_substitutions.is_empty() {
        return Ok(());
    }

    for ty in state.provisional.expr_types.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for ty in state.provisional.expr_memory_places.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for ty in state.provisional.stmt_types.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for ty in state.provisional.pat_types.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty)?;
    }
    for fact in state.provisional.symbol_types.values_mut() {
        substitute_symbol_fact(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for fact in state.provisional.item_signatures.values_mut() {
        substitute_item_signature(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for fact in state.provisional.try_facts.values_mut() {
        fact.value_type =
            substitute_type(ctx, &substitutions, &named_substitutions, fact.value_type)?;
        fact.result_type =
            substitute_type(ctx, &substitutions, &named_substitutions, fact.result_type)?;
        if let Some(error) = &mut fact.target_error {
            *error = substitute_type(ctx, &substitutions, &named_substitutions, *error)?;
        }
    }
    for fact in state
        .provisional
        .index_facts
        .values_mut()
        .chain(state.solver_report.index_facts.values_mut())
    {
        substitute_index_fact(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for fact in state
        .provisional
        .slice_facts
        .values_mut()
        .chain(state.solver_report.slice_facts.values_mut())
    {
        substitute_slice_fact(ctx, &substitutions, &named_substitutions, fact)?;
    }
    for error in state.solver_report.checked_index_errors.values_mut() {
        *error = substitute_type(ctx, &substitutions, &named_substitutions, *error)?;
    }
    for fact in state.solver_report.generic_instantiations.values_mut() {
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
        crate::SymbolTypeFact::Flow { signature }
        | crate::SymbolTypeFact::Agent { signature }
        | crate::SymbolTypeFact::Tool { signature } => {
            substitute_callable_signature(ctx, substitutions, named, signature)?;
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
        crate::ItemSignature::Flow(signature)
        | crate::ItemSignature::Agent(signature)
        | crate::ItemSignature::Tool(signature) => {
            substitute_callable_signature(ctx, substitutions, named, signature)?;
        }
        crate::ItemSignature::TopLevelLet(signature) => {
            signature.ty = substitute_type(ctx, substitutions, named, signature.ty)?;
        }
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
