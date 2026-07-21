use etas_core::{Diagnostic, SourceId, TextSize, TypeDiagnosticCode};

use super::state::BodyPipelineState;

pub fn run(ctx: &mut crate::pipeline::context::TypePipelineContext<'_>, state: BodyPipelineState) {
    let mut state = state;
    apply_solver_substitutions(ctx, &mut state);
    state
        .provisional
        .expr_types
        .extend(state.solver_report.inferred_expr_types.clone());
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
        .item_signatures
        .extend(state.provisional.item_signatures);
}

fn apply_solver_substitutions(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) {
    let substitutions = state.solver_report.substitutions.clone();
    let named_substitutions = state.solver_report.named_substitutions.clone();
    if substitutions.iter().next().is_none() && named_substitutions.is_empty() {
        return;
    }

    for ty in state.provisional.expr_types.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty);
    }
    for ty in state.provisional.stmt_types.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty);
    }
    for ty in state.provisional.pat_types.values_mut() {
        *ty = substitute_type(ctx, &substitutions, &named_substitutions, *ty);
    }
    for fact in state.provisional.symbol_types.values_mut() {
        substitute_symbol_fact(ctx, &substitutions, &named_substitutions, fact);
    }
}

fn substitute_symbol_fact(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named_substitutions: &std::collections::HashMap<String, crate::TypeId>,
    fact: &mut crate::SymbolTypeFact,
) {
    match fact {
        crate::SymbolTypeFact::Param { ty }
        | crate::SymbolTypeFact::Local { ty, .. }
        | crate::SymbolTypeFact::Field { ty }
        | crate::SymbolTypeFact::Value { ty }
        | crate::SymbolTypeFact::TopLevelLet { ty, .. } => {
            *ty = substitute_type(ctx, substitutions, named_substitutions, *ty);
        }
        crate::SymbolTypeFact::Flow { signature }
        | crate::SymbolTypeFact::Agent { signature }
        | crate::SymbolTypeFact::Tool { signature } => {
            for param in &mut signature.params {
                *param = substitute_type(ctx, substitutions, named_substitutions, *param);
            }
            signature.output =
                substitute_type(ctx, substitutions, named_substitutions, signature.output);
            if let Some(row) = &mut signature.effects {
                substitute_effect_row(ctx, substitutions, named_substitutions, row);
            }
            if let Some(row) = &mut signature.requested_actions {
                substitute_effect_row(ctx, substitutions, named_substitutions, row);
            }
        }
        _ => {}
    }
}

fn substitute_effect_row(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named_substitutions: &std::collections::HashMap<String, crate::TypeId>,
    row: &mut crate::EffectRowRef,
) {
    for effect in &mut row.effects {
        for arg in &mut effect.args {
            if let crate::EffectArgRef::Type(ty) = arg {
                *ty = substitute_type(ctx, substitutions, named_substitutions, *ty);
            }
        }
    }
}

fn substitute_type(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named_substitutions: &std::collections::HashMap<String, crate::TypeId>,
    ty: crate::TypeId,
) -> crate::TypeId {
    substitute_type_inner(ctx, substitutions, named_substitutions, ty, &mut Vec::new())
}

fn substitute_type_inner(
    ctx: &mut crate::pipeline::context::TypePipelineContext<'_>,
    substitutions: &crate::Substitution,
    named_substitutions: &std::collections::HashMap<String, crate::TypeId>,
    ty: crate::TypeId,
    stack: &mut Vec<crate::TypeId>,
) -> crate::TypeId {
    if stack.contains(&ty) {
        let described = stack
            .iter()
            .copied()
            .chain(std::iter::once(ty))
            .map(|id| format!("{id:?}={:?}", ctx.interner.store().get(id)))
            .collect::<Vec<_>>();
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            etas_core::Span::empty(SourceId(0), TextSize::ZERO),
            format!("cyclic type substitution: {}", described.join(" -> ")),
        ));
        return ty;
    }
    stack.push(ty);
    let Some(ty_data) = ctx.interner.store().get(ty).cloned() else {
        stack.pop();
        return ty;
    };
    let substituted = match ty_data {
        crate::Type::Var(var) => substitutions
            .get(var)
            .filter(|replacement| *replacement != ty)
            .map(|ty| substitute_type_inner(ctx, substitutions, named_substitutions, ty, stack))
            .unwrap_or(ty),
        crate::Type::Named(name) if is_schematic_type_variable(&name.name) => named_substitutions
            .get(&name.name)
            .copied()
            .filter(|replacement| *replacement != ty)
            .map(|ty| substitute_type_inner(ctx, substitutions, named_substitutions, ty, stack))
            .unwrap_or(ty),
        crate::Type::Array(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Array(inner))
        }
        crate::Type::List(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::List(inner))
        }
        crate::Type::Set(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Set(inner))
        }
        crate::Type::Slice(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Slice(inner))
        }
        crate::Type::Option(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Option(inner))
        }
        crate::Type::Message(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Message(inner))
        }
        crate::Type::Range { index } => {
            let index =
                substitute_type_inner(ctx, substitutions, named_substitutions, index, stack);
            ctx.interner.intern(crate::Type::Range { index })
        }
        crate::Type::Map { key, value } => {
            let key = substitute_type_inner(ctx, substitutions, named_substitutions, key, stack);
            let value =
                substitute_type_inner(ctx, substitutions, named_substitutions, value, stack);
            ctx.interner.intern(crate::Type::Map { key, value })
        }
        crate::Type::Result { ok, err } => {
            let ok = substitute_type_inner(ctx, substitutions, named_substitutions, ok, stack);
            let err = substitute_type_inner(ctx, substitutions, named_substitutions, err, stack);
            ctx.interner.intern(crate::Type::Result { ok, err })
        }
        crate::Type::Tuple(elements) => {
            let elements = elements
                .into_iter()
                .map(|ty| substitute_type_inner(ctx, substitutions, named_substitutions, ty, stack))
                .collect();
            ctx.interner.intern(crate::Type::Tuple(elements))
        }
        crate::Type::Record(record) => {
            let fields = record
                .fields
                .into_iter()
                .map(|field| crate::FieldType {
                    name: field.name,
                    ty: substitute_type_inner(
                        ctx,
                        substitutions,
                        named_substitutions,
                        field.ty,
                        stack,
                    ),
                })
                .collect();
            ctx.interner
                .intern(crate::Type::Record(crate::RecordType { fields }))
        }
        crate::Type::Nominal(mut nominal) => {
            nominal.representation = nominal.representation.map(|representation| {
                substitute_type_inner(
                    ctx,
                    substitutions,
                    named_substitutions,
                    representation,
                    stack,
                )
            });
            ctx.interner.intern(crate::Type::Nominal(nominal))
        }
        crate::Type::Function(mut flow) => {
            flow.input = flow
                .input
                .into_iter()
                .map(|ty| substitute_type_inner(ctx, substitutions, named_substitutions, ty, stack))
                .collect();
            flow.output =
                substitute_type_inner(ctx, substitutions, named_substitutions, flow.output, stack);
            ctx.interner.intern(crate::Type::Function(flow))
        }
        crate::Type::Handler(mut handler) => {
            handler.result = handler.result.map(|result| {
                substitute_type_inner(ctx, substitutions, named_substitutions, result, stack)
            });
            ctx.interner.intern(crate::Type::Handler(handler))
        }
        crate::Type::Trust { wrapper, inner } => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Trust { wrapper, inner })
        }
        crate::Type::Schema(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::Schema(inner))
        }
        crate::Type::MemorySelection(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::MemorySelection(inner))
        }
        crate::Type::Store { key, value } => {
            let key = substitute_type_inner(ctx, substitutions, named_substitutions, key, stack);
            let value =
                substitute_type_inner(ctx, substitutions, named_substitutions, value, stack);
            ctx.interner.intern(crate::Type::Store { key, value })
        }
        crate::Type::MemoryRegion(inner) => {
            let inner =
                substitute_type_inner(ctx, substitutions, named_substitutions, inner, stack);
            ctx.interner.intern(crate::Type::MemoryRegion(inner))
        }
        crate::Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema }) => {
            let schema =
                substitute_type_inner(ctx, substitutions, named_substitutions, schema, stack);
            ctx.interner.intern(crate::Type::ResourceHandle(
                crate::ResourceHandleType::MemoryRegion { schema },
            ))
        }
        crate::Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature }) => {
            let signature =
                substitute_type_inner(ctx, substitutions, named_substitutions, signature, stack);
            ctx.interner.intern(crate::Type::ResourceHandle(
                crate::ResourceHandleType::ExternalTool { signature },
            ))
        }
        crate::Type::ResourceHandle(crate::ResourceHandleType::Other { name, args }) => {
            let args = args
                .into_iter()
                .map(|ty| substitute_type_inner(ctx, substitutions, named_substitutions, ty, stack))
                .collect();
            ctx.interner.intern(crate::Type::ResourceHandle(
                crate::ResourceHandleType::Other { name, args },
            ))
        }
        crate::Type::Applied { constructor, args } => {
            let args = args
                .into_iter()
                .map(|ty| substitute_type_inner(ctx, substitutions, named_substitutions, ty, stack))
                .collect();
            ctx.interner
                .intern(crate::Type::Applied { constructor, args })
        }
        _ => ty,
    };
    stack.pop();
    substituted
}

fn is_schematic_type_variable(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
}
