use std::collections::HashMap;

use etas_hir::{HirArg, HirExpr, HirGenericArg, ResolveResult, SymbolDef};

use crate::{
    AssignabilityReason, CallableGenericArg, ConstraintOrigin, FieldType, FlowType, HandlerType,
    MemoryPlaceType, RecordType, ResourceHandleType, SymbolTypeFact, Type, TypeConstraint, TypeId,
    lower::effect_row::lower_effect_row,
    lower::type_ref::lower_type_ref,
    pipeline::{
        body::collect::{
            expr::{callable_candidate_from_fact, collect_expr, value_type_from_fact},
            std_member::{
                raw_std_member_value_type_for_symbol, raw_std_qualified_path_value_type,
                std_member_callable_signature_for_symbol, std_member_value_type_for_symbol,
                std_method_candidates, std_qualified_path_callable_candidate,
                std_qualified_path_value_type,
            },
        },
        context::BodyCollectContext,
    },
    solver::TypeUnifier,
};

pub fn collect_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: etas_hir::HirExprId,
    callee: etas_hir::HirExprId,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    let output = collect_call_inner(ctx, call, callee, generic_args, args, span, expected)?;
    super::handler_request::record_forwarded_call(ctx, call, callee);
    Ok(output)
}

fn collect_call_inner<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: etas_hir::HirExprId,
    callee: etas_hir::HirExprId,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    super::agent_method::validate_default_call_access(ctx, callee, span);
    if let Some(output) = super::agent_method::collect_agent_path_call(
        ctx,
        call,
        callee,
        generic_args,
        args,
        expected,
    )? {
        return Ok(output);
    }
    if let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] {
        if let ResolveResult::Resolved(symbol) = path.resolution {
            let symbol = ctx
                .ctx
                .symbols
                .canonical_symbol(ctx.ctx.hir, symbol)
                .unwrap_or(symbol);
            let facts = ctx
                .ctx
                .signature_base
                .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
            if facts.agent_methods.contains_key(&symbol) {
                super::agent_method::validate_method_access(ctx, symbol, span);
                ctx.state
                    .provisional
                    .agent_method_calls
                    .insert(call, symbol);
            }
        }
        if super::enum_variant::declaration(ctx, path)
            .is_some_and(|variant| variant.field_names.is_some())
        {
            super::enum_variant::invalid(
                ctx,
                span,
                "named-field variant requires brace construction",
            );
        }
    }
    let checked_generic_args = lower_callable_generic_args(ctx, generic_args);
    let callable_signature = callee_callable_signature(ctx, callee);
    validate_generic_args(
        ctx,
        callable_signature
            .as_ref()
            .map(|signature| signature.generic_params.as_slice()),
        generic_args,
        span,
    );
    let type_generic_args = generic_args
        .iter()
        .filter_map(|arg| match arg {
            HirGenericArg::Type(ty) => lower_type_ref(ctx.ctx, *ty),
            HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => None,
        })
        .collect::<Vec<_>>();
    if let Some(output) = collect_std_qualified_call(
        ctx,
        call,
        callee,
        &checked_generic_args,
        args,
        span,
        expected,
    )? {
        return Ok(output);
    }
    if let Some(output) = collect_partially_resolved_method_call(
        ctx,
        call,
        callee,
        &checked_generic_args,
        args,
        span,
        expected,
    )? {
        return Ok(output);
    }
    if let Some(output) = collect_standard_variant_constructor_call(
        ctx,
        callee,
        &type_generic_args,
        args,
        span,
        expected,
    )? {
        if generic_args
            .iter()
            .any(|arg| matches!(arg, HirGenericArg::EffectRow(_)))
        {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span,
                message: "variant constructors require type arguments, not effect-row arguments"
                    .into(),
            });
        }
        return Ok(output);
    }
    let callable_candidate = collect_callable_callee(ctx, callee, callable_signature.is_some())?;
    let generic_params = callable_candidate
        .as_ref()
        .map(|candidate| candidate.generic_params.clone())
        .unwrap_or_default();
    let constructor = collect_type_constructor_callee(ctx, callee, &type_generic_args, span);
    let checked_generic_args = if constructor.is_some() {
        if generic_args
            .iter()
            .any(|arg| matches!(arg, HirGenericArg::EffectRow(_)))
        {
            super::enum_variant::invalid(
                ctx,
                span,
                "type constructors require type arguments, not effect-row arguments",
            );
        }
        Vec::new()
    } else {
        checked_generic_args
    };
    let callee_ty = if let Some(ty) = constructor {
        ctx.record_expr_type(callee, ty)
    } else if let Some(candidate) = callable_candidate {
        candidate.ty
    } else {
        collect_expr(ctx, callee, None)?
    };
    let signature_hint = super::callable_instantiation::signature_hints(
        ctx,
        callee_ty,
        &generic_params,
        &checked_generic_args,
        span,
    );
    let arg_tys = collect_call_args(
        ctx,
        args,
        signature_hint.as_ref().map(|flow| flow.input.as_slice()),
    )?;
    let hinted_callee = signature_hint
        .map(|flow| ctx.ctx.interner.intern(Type::Function(flow)))
        .unwrap_or(callee_ty);
    let inferred = infer_imported_std_call_output_from_args(ctx, callee, &arg_tys)
        .or_else(|| callable_output_for_expression(ctx, hinted_callee));
    let inferred = expected.filter(|_| !generic_params.is_empty()).or(inferred);
    let output = select_call_output(ctx, inferred, expected, span);
    ctx.validate(crate::ValidationRequest::CallableArity {
        callee_ty,
        arg_count: arg_tys.len(),
        span,
    });
    ctx.emit(TypeConstraint::Callable {
        call: Some(call),
        callee: callee_ty,
        generic_params,
        generic_args: checked_generic_args,
        arg_exprs: call_arg_exprs(args),
        args: arg_tys,
        output,
        origin: ConstraintOrigin { span },
    });
    Ok(output)
}

fn collect_std_qualified_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: etas_hir::HirExprId,
    callee: etas_hir::HirExprId,
    checked_generic_args: &[CallableGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let type_generic_args = checked_generic_args
        .iter()
        .filter_map(|arg| match arg {
            CallableGenericArg::Type(ty) => Some(*ty),
            CallableGenericArg::EffectRow(_) => None,
        })
        .collect::<Vec<_>>();
    let type_generic_args = type_generic_args.as_slice();
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return Ok(None);
    };
    let candidate = std_qualified_path_callable_candidate(ctx, path, true);
    let (callee_ty, generic_params) = if let Some(candidate) = candidate {
        (candidate.ty, candidate.generic_params)
    } else {
        let ty = if type_generic_args.is_empty() {
            std_qualified_path_value_type(ctx, path)
        } else {
            raw_std_qualified_path_value_type(ctx, path)
        };
        let Some(ty) = ty else {
            return Ok(None);
        };
        (ty, Vec::new())
    };
    let expected_inputs = super::callable_instantiation::argument_hints(
        ctx,
        callee_ty,
        &generic_params,
        checked_generic_args,
        span,
    );
    let arg_tys = collect_call_args(ctx, args, expected_inputs.as_deref())?;
    let inferred = infer_std_call_output_from_args(ctx, path, &arg_tys)
        .or_else(|| {
            specialized_callable_output_for_args(ctx, callee_ty, type_generic_args, &arg_tys, span)
        })
        .or_else(|| callable_output_for_expression(ctx, callee_ty));
    let inferred = expected.filter(|_| !generic_params.is_empty()).or(inferred);
    let output = select_call_output(ctx, inferred, expected, span);
    ctx.validate(crate::ValidationRequest::CallableArity {
        callee_ty,
        arg_count: arg_tys.len(),
        span,
    });
    ctx.emit(TypeConstraint::Callable {
        call: Some(call),
        callee: callee_ty,
        generic_params,
        generic_args: checked_generic_args.to_vec(),
        arg_exprs: call_arg_exprs(args),
        args: arg_tys,
        output,
        origin: ConstraintOrigin { span },
    });
    Ok(Some(output))
}

fn infer_std_call_output_from_args<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    path: &etas_hir::ResolvedPath,
    args: &[TypeId],
) -> Option<TypeId> {
    let name = path.segments.last()?.name.as_str();
    match (name, args) {
        ("unwrap", [arg]) => match ctx.ctx.interner.store().get(*arg) {
            Some(Type::Option(inner)) => Some(*inner),
            Some(Type::Result { ok, .. }) => Some(*ok),
            _ => None,
        },
        _ => None,
    }
}

fn infer_imported_std_call_output_from_args<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    callee: etas_hir::HirExprId,
    args: &[TypeId],
) -> Option<TypeId> {
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return None;
    };
    let symbol = resolved_callee_symbol(path)?;
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    let symbol = ctx.ctx.hir.symbols.get(symbol)?;
    let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
        return None;
    };
    match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["std", "option", "unwrap"] => infer_unwrap_output_from_args(ctx, args),
        _ => None,
    }
}

fn infer_unwrap_output_from_args<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    args: &[TypeId],
) -> Option<TypeId> {
    let [arg] = args else {
        return None;
    };
    match ctx.ctx.interner.store().get(*arg) {
        Some(Type::Option(inner)) => Some(*inner),
        Some(Type::Result { ok, .. }) => Some(*ok),
        _ => None,
    }
}

fn collect_callable_callee<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: etas_hir::HirExprId,
    instantiate_schematics: bool,
) -> Result<Option<crate::CallableCandidate>, E> {
    ctx.checkpoint()?;
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return Ok(None);
    };
    let Some(symbol) = resolved_callee_symbol(path) else {
        return Ok(None);
    };
    let Some(fact) = ctx.ctx.symbol_fact(symbol).cloned() else {
        return Ok(None);
    };
    let value_type = match &fact {
        SymbolTypeFact::Agent { signature } => Some(signature.component),
        _ => None,
    };
    let Some(candidate) = callable_candidate_from_fact(ctx, fact, instantiate_schematics) else {
        return Ok(None);
    };
    ctx.record_expr_type(callee, value_type.unwrap_or(candidate.ty));
    Ok(Some(candidate))
}

fn callee_callable_signature<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    callee: etas_hir::HirExprId,
) -> Option<crate::CallableSignature> {
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return None;
    };
    let symbol = resolved_callee_symbol(path)?;
    let fact = ctx
        .state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))?;
    match fact {
        SymbolTypeFact::Flow { signature } | SymbolTypeFact::Tool { signature } => {
            Some(signature.clone())
        }
        SymbolTypeFact::Agent { signature } => signature.callable().cloned(),
        _ => None,
    }
}

fn collect_type_constructor_callee<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: etas_hir::HirExprId,
    type_generic_args: &[TypeId],
    span: etas_core::Span,
) -> Option<TypeId> {
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return None;
    };
    let symbol = resolved_callee_symbol(path)?;
    let fact = ctx.ctx.symbol_fact(symbol)?.clone();
    let alias = matches!(fact, SymbolTypeFact::TypeAlias { .. });
    let target = super::nominal_constructor::target(ctx, fact, type_generic_args, span)?;
    if alias {
        ctx.emit(TypeConstraint::Constructible {
            target,
            origin: ConstraintOrigin { span },
        });
        let constructor = match ctx.ctx.interner.store().get(target) {
            Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
            _ => target,
        };
        if matches!(
            ctx.ctx.interner.store().get(constructor),
            Some(Type::Nominal(_))
        ) {
            return Some(target);
        }
        Some(ctx.ctx.interner.intern(Type::Function(FlowType {
            input: vec![target],
            output: target,
            effects: None,
        })))
    } else {
        Some(target)
    }
}

fn resolved_callee_symbol(path: &etas_hir::ResolvedPath) -> Option<etas_hir::SymbolId> {
    match &path.resolution {
        ResolveResult::Resolved(symbol) => Some(*symbol),
        ResolveResult::PartiallyResolved(partial)
            if partial.remaining.is_empty()
                && partial.reason
                    == etas_hir::PartialResolutionReason::MemberRequiresTypeChecking =>
        {
            partial.resolved_prefix
        }
        _ => None,
    }
}

fn collect_partially_resolved_method_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: etas_hir::HirExprId,
    callee: etas_hir::HirExprId,
    checked_generic_args: &[CallableGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let type_generic_args = checked_generic_args
        .iter()
        .filter_map(|arg| match arg {
            CallableGenericArg::Type(ty) => Some(*ty),
            CallableGenericArg::EffectRow(_) => None,
        })
        .collect::<Vec<_>>();
    let type_generic_args = type_generic_args.as_slice();
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return Ok(None);
    };
    let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
        return Ok(None);
    };
    let Some(receiver_symbol) = partial.resolved_prefix else {
        return Ok(None);
    };
    let [method] = partial.remaining.as_slice() else {
        return Ok(None);
    };
    let candidates = super::std_member::std_type_member_candidates_for_symbol(
        ctx,
        receiver_symbol,
        method,
        checked_generic_args.is_empty(),
    );
    if candidates.len() > 1
        || candidates.iter().any(|candidate| {
            matches!(
                candidate.operation,
                crate::CallableCandidateOperation::StdIntrinsic {
                    has_receiver: false,
                    ..
                }
            )
        })
    {
        return super::std_type_member::collect(
            ctx,
            super::std_type_member::Call {
                expr: call,
                callee: Some(callee),
                method,
                candidates,
                generic_args: checked_generic_args,
                args,
                span,
                expected,
            },
        )
        .map(Some);
    }
    let generic_params = std_member_callable_signature_for_symbol(ctx, receiver_symbol, method)
        .map(|signature| signature.generic_params)
        .unwrap_or_default();
    let std_member = if type_generic_args.is_empty() && generic_params.is_empty() {
        std_member_value_type_for_symbol(ctx, receiver_symbol, method)
    } else {
        raw_std_member_value_type_for_symbol(ctx, receiver_symbol, method)
    };
    if let Some(callee_ty) = std_member {
        let expected_inputs = callable_input_types(ctx, callee_ty, type_generic_args, span);
        let arg_tys = collect_call_args(ctx, args, expected_inputs.as_deref())?;
        let inferred =
            specialized_callable_output_for_args(ctx, callee_ty, type_generic_args, &arg_tys, span)
                .or_else(|| callable_output_for_expression(ctx, callee_ty));
        let inferred = expected.filter(|_| !generic_params.is_empty()).or(inferred);
        let output = select_call_output(ctx, inferred, expected, span);
        ctx.validate(crate::ValidationRequest::CallableArity {
            callee_ty,
            arg_count: arg_tys.len(),
            span,
        });
        ctx.emit(TypeConstraint::Callable {
            call: Some(call),
            callee: callee_ty,
            generic_params,
            generic_args: checked_generic_args.to_vec(),
            arg_exprs: call_arg_exprs(args),
            args: arg_tys,
            output,
            origin: ConstraintOrigin { span },
        });
        return Ok(Some(output));
    }
    let mut candidates = std_method_candidates(ctx, method);
    candidates.extend(super::agent_method::deferred_candidates(
        ctx,
        method,
        checked_generic_args,
        span,
    ));
    if candidates.is_empty() && !super::agent_method::has_method_name(ctx, method) {
        return Ok(None);
    }
    let Some(receiver_fact) = ctx
        .state
        .provisional
        .symbol_types
        .get(&receiver_symbol)
        .or_else(|| ctx.ctx.symbol_fact(receiver_symbol))
        .cloned()
    else {
        return Ok(None);
    };
    let Some(receiver_ty) = value_type_from_fact(ctx, receiver_fact) else {
        return Ok(None);
    };
    let mut arg_tys = Vec::with_capacity(args.len() + 1);
    arg_tys.push(receiver_ty);
    let actual = args
        .iter()
        .map(|arg| match arg {
            HirArg::Positional(expr) => collect_expr(ctx, *expr, None),
            HirArg::Named { value, .. } => collect_expr(ctx, *value, None),
        })
        .collect::<Result<Vec<_>, E>>()?;
    arg_tys.extend(actual);
    let inferred =
        specialized_method_output_for_args(ctx, &candidates, type_generic_args, &arg_tys, span);
    // Overload outputs are contextual hints until the method solver selects a
    // checked candidate, unlike the result of a resolved callable signature.
    let output = expected
        .or(inferred)
        .unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::MethodCall {
        expr: call,
        method: method.clone(),
        access_module: ctx.ctx.hir.item_module(ctx.state.item),
        candidates,
        generic_args: checked_generic_args.to_vec(),
        arg_exprs: std::iter::once(None).chain(call_arg_exprs(args)).collect(),
        args: arg_tys,
        output,
        origin: ConstraintOrigin { span },
    });
    Ok(Some(output))
}

pub(super) fn select_call_output<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    inferred: Option<TypeId>,
    expected: Option<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    let output = match inferred {
        Some(ty) if !matches!(ctx.ctx.interner.store().get(ty), Some(Type::Var(_))) => ty,
        _ => expected
            .or(inferred)
            .unwrap_or_else(|| ctx.fresh_type_var()),
    };
    if let Some(expected) = expected
        && expected != output
    {
        ctx.emit(TypeConstraint::Assignable {
            from: output,
            to: expected,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Other,
        });
    }
    output
}

pub fn specialized_callable_output_for_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee_ty: TypeId,
    explicit_generic_args: &[TypeId],
    arg_tys: &[TypeId],
    span: etas_core::Span,
) -> Option<TypeId> {
    let Type::Function(flow) = ctx.ctx.interner.store().get(callee_ty).cloned()? else {
        return None;
    };
    if flow.input.len() != arg_tys.len() {
        return None;
    }
    let mut input = flow.input;
    let mut output = flow.output;
    if !explicit_generic_args.is_empty() {
        let substitutions = explicit_generic_substitutions(
            ctx,
            input
                .iter()
                .copied()
                .chain(std::iter::once(output))
                .collect::<Vec<_>>()
                .as_slice(),
            explicit_generic_args,
        );
        if !substitutions.is_empty() {
            input = input
                .into_iter()
                .map(|ty| substitute_named_or_report(ctx, ty, &substitutions, span))
                .collect();
            output = substitute_named_or_report(ctx, output, &substitutions, span);
        }
    }

    let mut unifier = TypeUnifier::new(ctx.ctx.interner.store());
    for (expected, actual) in input.iter().copied().zip(arg_tys.iter().copied()) {
        if unifier.unify(expected, actual).is_err() {
            return None;
        }
    }
    let substitution = unifier.substitution().clone();
    Some(apply_type_var_substitution(ctx, output, &substitution))
}

pub fn specialized_method_output_for_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    candidates: &[crate::CallableCandidate],
    explicit_generic_args: &[TypeId],
    arg_tys: &[TypeId],
    span: etas_core::Span,
) -> Option<TypeId> {
    let mut selected = None;
    for candidate in candidates {
        let Some(output) = specialized_callable_output_for_args(
            ctx,
            candidate.ty,
            explicit_generic_args,
            arg_tys,
            span,
        ) else {
            continue;
        };
        match selected {
            None => selected = Some(output),
            Some(existing) if existing == output => {}
            Some(_) => return None,
        }
    }
    selected
}

fn explicit_generic_substitutions<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    types: &[TypeId],
    explicit_generic_args: &[TypeId],
) -> HashMap<String, TypeId> {
    let mut names = Vec::new();
    for ty in types {
        collect_schematic_type_names(ctx, *ty, &mut names);
    }
    names
        .into_iter()
        .zip(explicit_generic_args.iter().copied())
        .collect()
}

fn collect_schematic_type_names<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    ty: TypeId,
    out: &mut Vec<String>,
) {
    match ctx.ctx.interner.store().get(ty) {
        Some(Type::Named(name)) if is_schematic_type_variable_name(&name.name) => {
            if !out.contains(&name.name) {
                out.push(name.name.clone());
            }
        }
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner })
        | Some(Type::Trust { inner, .. }) => collect_schematic_type_names(ctx, *inner, out),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            collect_schematic_type_names(ctx, *key, out);
            collect_schematic_type_names(ctx, *value, out);
        }
        Some(Type::Result { ok, err }) => {
            collect_schematic_type_names(ctx, *ok, out);
            collect_schematic_type_names(ctx, *err, out);
        }
        Some(Type::Tuple(elements)) => {
            for element in elements {
                collect_schematic_type_names(ctx, *element, out);
            }
        }
        Some(Type::Record(record)) => {
            for field in &record.fields {
                collect_schematic_type_names(ctx, field.ty, out);
            }
        }
        Some(Type::Function(flow)) => {
            for input in &flow.input {
                collect_schematic_type_names(ctx, *input, out);
            }
            collect_schematic_type_names(ctx, flow.output, out);
        }
        Some(Type::Applied { args, .. }) => {
            for arg in args {
                collect_schematic_type_names(ctx, *arg, out);
            }
        }
        _ => {}
    }
}

fn is_schematic_type_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
}

fn apply_type_var_substitution<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: TypeId,
    substitution: &crate::Substitution,
) -> TypeId {
    let Some(ty_data) = ctx.ctx.interner.store().get(ty).cloned() else {
        return ty;
    };
    match ty_data {
        Type::Var(var) => substitution.get(var).unwrap_or(ty),
        Type::Array(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Array(inner))
        }
        Type::List(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::List(inner))
        }
        Type::Map { key, value } => {
            let key = apply_type_var_substitution(ctx, key, substitution);
            let value = apply_type_var_substitution(ctx, value, substitution);
            ctx.ctx.interner.intern(Type::Map { key, value })
        }
        Type::Set(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Set(inner))
        }
        Type::Range { index } => {
            let index = apply_type_var_substitution(ctx, index, substitution);
            ctx.ctx.interner.intern(Type::Range { index })
        }
        Type::Slice(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Slice(inner))
        }
        Type::Option(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Option(inner))
        }
        Type::Result { ok, err } => {
            let ok = apply_type_var_substitution(ctx, ok, substitution);
            let err = apply_type_var_substitution(ctx, err, substitution);
            ctx.ctx.interner.intern(Type::Result { ok, err })
        }
        Type::Record(record) => {
            let fields = record
                .fields
                .into_iter()
                .map(|field| FieldType {
                    name: field.name,
                    ty: apply_type_var_substitution(ctx, field.ty, substitution),
                })
                .collect();
            ctx.ctx.interner.intern(Type::Record(RecordType { fields }))
        }
        Type::Tuple(elements) => {
            let elements = elements
                .into_iter()
                .map(|element| apply_type_var_substitution(ctx, element, substitution))
                .collect();
            ctx.ctx.interner.intern(Type::Tuple(elements))
        }
        Type::Function(flow) => {
            let input = flow
                .input
                .into_iter()
                .map(|input| apply_type_var_substitution(ctx, input, substitution))
                .collect();
            let output = apply_type_var_substitution(ctx, flow.output, substitution);
            ctx.ctx.interner.intern(Type::Function(FlowType {
                input,
                output,
                effects: flow.effects,
            }))
        }
        Type::Handler(handler) => {
            let result = handler
                .result
                .map(|result| apply_type_var_substitution(ctx, result, substitution));
            ctx.ctx.interner.intern(Type::Handler(HandlerType {
                handled: handler.handled,
                produced: handler.produced,
                result,
            }))
        }
        Type::Applied { constructor, args } => {
            let args = args
                .into_iter()
                .map(|arg| apply_type_var_substitution(ctx, arg, substitution))
                .collect();
            ctx.ctx.interner.intern(Type::Applied { constructor, args })
        }
        Type::Refined { base, predicate } => {
            let base = apply_type_var_substitution(ctx, base, substitution);
            ctx.ctx.interner.intern(Type::Refined { base, predicate })
        }
        Type::Trust { wrapper, inner } => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Trust { wrapper, inner })
        }
        Type::Schema(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Schema(inner))
        }
        Type::Message(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::Message(inner))
        }
        Type::MemorySelection(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::MemorySelection(inner))
        }
        Type::Store { key, value } => {
            let key = apply_type_var_substitution(ctx, key, substitution);
            let value = apply_type_var_substitution(ctx, value, substitution);
            ctx.ctx.interner.intern(Type::Store { key, value })
        }
        Type::MemoryRegion(inner) => {
            let inner = apply_type_var_substitution(ctx, inner, substitution);
            ctx.ctx.interner.intern(Type::MemoryRegion(inner))
        }
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
            let schema = apply_type_var_substitution(ctx, schema, substitution);
            ctx.ctx
                .interner
                .intern(Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                    schema,
                }))
        }
        Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
            let signature = apply_type_var_substitution(ctx, signature, substitution);
            ctx.ctx
                .interner
                .intern(Type::ResourceHandle(ResourceHandleType::ExternalTool {
                    signature,
                }))
        }
        Type::ResourceHandle(ResourceHandleType::Other { name, args }) => {
            let args = args
                .into_iter()
                .map(|arg| apply_type_var_substitution(ctx, arg, substitution))
                .collect();
            ctx.ctx
                .interner
                .intern(Type::ResourceHandle(ResourceHandleType::Other {
                    name,
                    args,
                }))
        }
        Type::Primitive(_)
        | Type::IntegerLiteral { .. }
        | Type::Enum(_)
        | Type::Named(_)
        | Type::Nominal(_)
        | Type::Prompt
        | Type::PromptPart
        | Type::MemoryPlace(MemoryPlaceType { .. }) => ty,
    }
}

fn collect_standard_variant_constructor_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: etas_hir::HirExprId,
    type_generic_args: &[TypeId],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[callee] else {
        return Ok(None);
    };
    if super::enum_variant::declaration(ctx, path).is_some() {
        return Ok(None);
    }
    let Some(name) = path.segments.last().map(|segment| segment.name.as_str()) else {
        return Ok(None);
    };
    Ok(match name {
        "Ok" => Some(collect_result_constructor_call(
            ctx,
            "Ok",
            ResultConstructor::Ok,
            type_generic_args,
            args,
            span,
            expected,
        )?),
        "Err" => Some(collect_result_constructor_call(
            ctx,
            "Err",
            ResultConstructor::Err,
            type_generic_args,
            args,
            span,
            expected,
        )?),
        "Some" => Some(collect_option_constructor_call(
            ctx,
            "Some",
            OptionConstructor::Some,
            type_generic_args,
            args,
            span,
            expected,
        )?),
        "None" => Some(collect_option_constructor_call(
            ctx,
            "None",
            OptionConstructor::None,
            type_generic_args,
            args,
            span,
            expected,
        )?),
        _ => None,
    })
}

#[derive(Clone, Copy)]
enum ResultConstructor {
    Ok,
    Err,
}

fn collect_result_constructor_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    constructor: ResultConstructor,
    type_generic_args: &[TypeId],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    if !type_generic_args.is_empty() && type_generic_args.len() != 2 {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span,
            message: format!(
                "{} constructor expects 2 type argument(s), got {}",
                name,
                type_generic_args.len()
            ),
        });
    }
    let expected_result = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
        Some(Type::Result { ok, err }) => Some((*ok, *err)),
        _ => None,
    });
    if expected_result.is_none() && type_generic_args.len() != 2 {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: format!(
                "{} constructor requires an expected Result<T, E> type or explicit <T, E> arguments",
                name
            ),
        });
    }
    let ok = type_generic_args
        .first()
        .copied()
        .or_else(|| expected_result.map(|(ok, _)| ok))
        .unwrap_or_else(|| ctx.fresh_type_var());
    let err = type_generic_args
        .get(1)
        .copied()
        .or_else(|| expected_result.map(|(_, err)| err))
        .unwrap_or_else(|| ctx.fresh_type_var());
    let result = ctx.ctx.interner.intern(Type::Result { ok, err });
    let output = expected.unwrap_or(result);
    ctx.emit(TypeConstraint::Assignable {
        from: result,
        to: output,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Other,
    });
    let payload = match constructor {
        ResultConstructor::Ok => ok,
        ResultConstructor::Err => err,
    };
    collect_constructor_call_args(ctx, name, args, &[payload], span)?;
    Ok(output)
}

#[derive(Clone, Copy)]
enum OptionConstructor {
    Some,
    None,
}

fn collect_option_constructor_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    constructor: OptionConstructor,
    type_generic_args: &[TypeId],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    if !type_generic_args.is_empty() && type_generic_args.len() != 1 {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span,
            message: format!(
                "{} constructor expects 1 type argument(s), got {}",
                name,
                type_generic_args.len()
            ),
        });
    }
    let expected_inner = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
        Some(Type::Option(inner)) => Some(*inner),
        _ => None,
    });
    let inner = type_generic_args
        .first()
        .copied()
        .or(expected_inner)
        .unwrap_or_else(|| ctx.fresh_type_var());
    let option = ctx.ctx.interner.intern(Type::Option(inner));
    let output = expected.unwrap_or(option);
    ctx.emit(TypeConstraint::Assignable {
        from: option,
        to: output,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Other,
    });
    match constructor {
        OptionConstructor::Some => collect_constructor_call_args(ctx, name, args, &[inner], span),
        OptionConstructor::None => collect_constructor_call_args(ctx, name, args, &[], span),
    }?;
    Ok(output)
}

fn collect_constructor_call_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    args: &[HirArg],
    expected: &[TypeId],
    span: etas_core::Span,
) -> Result<(), E> {
    ctx.checkpoint()?;
    if args.len() != expected.len() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span,
            message: format!(
                "{} constructor expects {} argument(s), got {}",
                name,
                expected.len(),
                args.len()
            ),
        });
    }
    for (arg, expected) in args.iter().zip(expected.iter().copied()) {
        let actual = match arg {
            HirArg::Positional(expr) => collect_expr(ctx, *expr, Some(expected)),
            HirArg::Named { value, .. } => collect_expr(ctx, *value, Some(expected)),
        }?;
        ctx.emit(TypeConstraint::Assignable {
            from: actual,
            to: expected,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Argument,
        });
    }
    for arg in args.iter().skip(expected.len()) {
        match arg {
            HirArg::Positional(expr) => {
                collect_expr(ctx, *expr, None)?;
            }
            HirArg::Named { value, .. } => {
                collect_expr(ctx, *value, None)?;
            }
        }
    }
    Ok(())
}

pub fn callable_output<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    callee_ty: TypeId,
) -> Option<TypeId> {
    match ctx.ctx.interner.store().get(callee_ty) {
        Some(crate::Type::Function(flow)) => Some(flow.output),
        _ => None,
    }
}

pub(super) fn callable_input_types<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee_ty: TypeId,
    explicit_generic_args: &[TypeId],
    span: etas_core::Span,
) -> Option<Vec<TypeId>> {
    let callee_ty = ctx
        .ctx
        .agent_signature(callee_ty)
        .and_then(|agent| agent.default_entry.as_ref())
        .map_or(callee_ty, |entry| entry.callable_type);
    let Type::Function(flow) = ctx.ctx.interner.store().get(callee_ty).cloned()? else {
        return None;
    };
    let input = flow.input;
    if explicit_generic_args.is_empty() {
        return Some(input);
    }
    let substitutions = explicit_generic_substitutions(
        ctx,
        input
            .iter()
            .copied()
            .chain(std::iter::once(flow.output))
            .collect::<Vec<_>>()
            .as_slice(),
        explicit_generic_args,
    );
    if substitutions.is_empty() {
        return Some(input);
    }
    Some(
        input
            .into_iter()
            .map(|ty| substitute_named_or_report(ctx, ty, &substitutions, span))
            .collect(),
    )
}

pub(super) fn substitute_named_or_report<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: TypeId,
    substitutions: &HashMap<String, TypeId>,
    span: etas_core::Span,
) -> TypeId {
    match crate::substitute_named_params(&mut ctx.ctx.interner, ty, substitutions) {
        Ok(ty) => ty,
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("callable type substitution failed: {error}"),
            });
            ctx.primitive(crate::PrimitiveType::Never)
        }
    }
}

pub(super) fn collect_call_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    args: &[HirArg],
    expected: Option<&[TypeId]>,
) -> Result<Vec<TypeId>, E> {
    ctx.checkpoint()?;
    args.iter()
        .enumerate()
        .map(|(index, arg)| {
            let expected = expected.and_then(|expected| expected.get(index).copied());
            match arg {
                HirArg::Positional(expr) => collect_expr(ctx, *expr, expected),
                HirArg::Named { value, .. } => collect_expr(ctx, *value, expected),
            }
        })
        .collect()
}

pub(super) fn call_arg_exprs(args: &[HirArg]) -> Vec<Option<etas_hir::HirExprId>> {
    args.iter()
        .map(|arg| {
            Some(match arg {
                HirArg::Positional(expr) => *expr,
                HirArg::Named { value, .. } => *value,
            })
        })
        .collect()
}

pub fn callable_output_for_expression<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee_ty: TypeId,
) -> Option<TypeId> {
    let callee_ty = ctx
        .ctx
        .agent_signature(callee_ty)
        .and_then(|agent| agent.default_entry.as_ref())
        .map_or(callee_ty, |entry| entry.callable_type);
    let output = callable_output(ctx, callee_ty)?;
    if contains_callable_generic_var(ctx, output) {
        Some(ctx.fresh_type_var())
    } else {
        Some(output)
    }
}

fn contains_callable_generic_var<E>(ctx: &BodyCollectContext<'_, '_, E>, ty: TypeId) -> bool {
    match ctx.ctx.interner.store().get(ty) {
        Some(Type::Var(_)) => true,
        Some(Type::Named(name)) => {
            let mut chars = name.name.chars();
            matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
        }
        Some(Type::Array(inner))
        | Some(Type::List(inner))
        | Some(Type::Set(inner))
        | Some(Type::Slice(inner))
        | Some(Type::Option(inner))
        | Some(Type::Message(inner))
        | Some(Type::Schema(inner))
        | Some(Type::MemorySelection(inner))
        | Some(Type::MemoryRegion(inner))
        | Some(Type::Range { index: inner })
        | Some(Type::Trust { inner, .. }) => contains_callable_generic_var(ctx, *inner),
        Some(Type::Map { key, value }) | Some(Type::Store { key, value }) => {
            contains_callable_generic_var(ctx, *key) || contains_callable_generic_var(ctx, *value)
        }
        Some(Type::Result { ok, err }) => {
            contains_callable_generic_var(ctx, *ok) || contains_callable_generic_var(ctx, *err)
        }
        Some(Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_callable_generic_var(ctx, *element)),
        Some(Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_callable_generic_var(ctx, *input))
                || contains_callable_generic_var(ctx, flow.output)
        }
        _ => false,
    }
}

fn validate_generic_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    generic_params: Option<&[crate::CallableGenericParam]>,
    generic_args: &[HirGenericArg],
    span: etas_core::Span,
) {
    let effect_row_args = generic_args
        .iter()
        .filter(|arg| matches!(arg, HirGenericArg::EffectRow(_)))
        .count();
    for arg in generic_args {
        if let HirGenericArg::Wildcard { span } = arg {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: *span,
                message: "wildcard generic argument is only valid as an effect/action selector"
                    .to_owned(),
            });
        }
    }
    if effect_row_args == 0 {
        for arg in generic_args {
            if let HirGenericArg::Type(ty) = arg {
                let _ = lower_type_ref(ctx.ctx, *ty);
            }
        }
        return;
    }

    for arg in generic_args {
        if let HirGenericArg::Type(ty) = arg {
            let _ = lower_type_ref(ctx.ctx, *ty);
        }
    }

    let Some(generic_params) = generic_params else {
        // Receiver-based calls resolve their declaration in the solver. An
        // absent signature here is not evidence of zero generic parameters.
        return;
    };
    let effect_param_count = generic_params
        .iter()
        .filter(|param| matches!(param.kind, crate::CallableGenericParamKind::Effect))
        .count();
    if effect_param_count < effect_row_args {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message:
                "effect-row generic argument is only valid for row-polymorphic callable parameters"
                    .to_owned(),
        });
    }
}

pub(super) fn lower_callable_generic_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    generic_args: &[HirGenericArg],
) -> Vec<CallableGenericArg> {
    generic_args
        .iter()
        .filter_map(|arg| match arg {
            HirGenericArg::Type(ty) => lower_type_ref(ctx.ctx, *ty).map(CallableGenericArg::Type),
            HirGenericArg::EffectRow(row) => Some(CallableGenericArg::EffectRow(lower_effect_row(
                ctx.ctx, row,
            ))),
            HirGenericArg::Wildcard { .. } => None,
        })
        .collect()
}
