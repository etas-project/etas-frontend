use std::collections::HashMap;

use etas_hir::{HirExpr, ImportAliasOrigin, ResolveResult, SymbolDef};
use etas_std::{FlowSourceMethodKind, StdDecl, StdRegistry, StdSymbol, StdType};

use crate::{
    CallableCandidate, CallableSignature, FlowType, SymbolTypeFact, Type, TypeId,
    lower::std::{lower_std_symbol, lower_std_type},
    pipeline::{
        body::collect::expr::{
            callable_candidate_from_fact, raw_value_type_from_fact, value_type_from_fact,
        },
        context::BodyCollectContext,
    },
    substitute_named_params,
};

pub fn std_qualified_path_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
) -> Option<TypeId> {
    std_qualified_path_value_type_with_instantiation(ctx, path, true)
}

pub fn raw_std_qualified_path_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
) -> Option<TypeId> {
    std_qualified_path_value_type_with_instantiation(ctx, path, false)
}

fn std_qualified_path_value_type_with_instantiation(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
    instantiate_schematics: bool,
) -> Option<TypeId> {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("std") {
        return None;
    }
    let registry = ctx.ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(&segments)?;
    let fact = lower_std_symbol(ctx.ctx, &registry, etas_hir::SymbolId(0), symbol);
    if instantiate_schematics {
        value_type_from_fact(ctx, fact)
    } else {
        raw_value_type_from_fact(ctx, fact)
    }
}

pub fn std_member_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: etas_hir::HirExprId,
    member: &str,
) -> Option<TypeId> {
    if let Some(ty) = std_source_type_member_value_type(ctx, base, member, true) {
        return Some(ty);
    }
    if let HirExpr::Path(path) = &ctx.ctx.hir.exprs[base]
        && let Some(ty) = std_qualified_member_value_type(ctx, path, member)
    {
        return Some(ty);
    }
    let base_symbol = match &ctx.ctx.hir.exprs[base] {
        HirExpr::Path(path) => match path.resolution {
            ResolveResult::Resolved(symbol) => symbol,
            _ => return None,
        },
        _ => return None,
    };
    let fact = std_member_fact(ctx, base_symbol, member)?;
    value_type_from_fact(ctx, fact)
}

fn std_qualified_member_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
    member: &str,
) -> Option<TypeId> {
    std_qualified_member_value_type_with_instantiation(ctx, path, member, true)
}

fn std_qualified_member_value_type_with_instantiation(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
    member: &str,
    instantiate_schematics: bool,
) -> Option<TypeId> {
    let mut segments = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("std") {
        return None;
    }
    segments.push(member);
    let registry = ctx.ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(&segments)?;
    let fact = lower_std_symbol(ctx.ctx, &registry, etas_hir::SymbolId(0), symbol);
    if instantiate_schematics {
        value_type_from_fact(ctx, fact)
    } else {
        raw_value_type_from_fact(ctx, fact)
    }
}

pub fn std_member_value_type_for_symbol(
    ctx: &mut BodyCollectContext<'_, '_>,
    base_symbol: etas_hir::SymbolId,
    member: &str,
) -> Option<TypeId> {
    if let Some(ty) = std_source_type_member_value_type_for_symbol(ctx, base_symbol, member, true) {
        return Some(ty);
    }
    let fact = std_member_fact(ctx, base_symbol, member)?;
    value_type_from_fact(ctx, fact)
}

pub fn raw_std_member_value_type_for_symbol(
    ctx: &mut BodyCollectContext<'_, '_>,
    base_symbol: etas_hir::SymbolId,
    member: &str,
) -> Option<TypeId> {
    if let Some(ty) = std_source_type_member_value_type_for_symbol(ctx, base_symbol, member, false)
    {
        return Some(ty);
    }
    let fact = std_member_fact(ctx, base_symbol, member)?;
    raw_value_type_from_fact(ctx, fact)
}

pub fn std_method_candidates(
    ctx: &mut BodyCollectContext<'_, '_>,
    method: &str,
) -> Vec<CallableCandidate> {
    let registry = ctx.ctx.std_registry.clone();
    registry
        .symbols()
        .filter(|symbol| std_symbol_is_value_method_candidate(symbol, method))
        .filter_map(|symbol| {
            let fact = lower_std_symbol(ctx.ctx, &registry, etas_hir::SymbolId(0), symbol);
            callable_candidate_from_fact(ctx, fact, true)
        })
        .collect()
}

pub fn raw_std_method_candidates(
    ctx: &mut BodyCollectContext<'_, '_>,
    method: &str,
) -> Vec<CallableCandidate> {
    let registry = ctx.ctx.std_registry.clone();
    registry
        .symbols()
        .filter(|symbol| std_symbol_is_value_method_candidate(symbol, method))
        .filter_map(|symbol| {
            let fact = lower_std_symbol(ctx.ctx, &registry, etas_hir::SymbolId(0), symbol);
            callable_candidate_from_fact(ctx, fact, false)
        })
        .collect()
}

pub fn std_qualified_path_callable_signature(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
) -> Option<CallableSignature> {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("std") {
        return None;
    }
    let registry = ctx.ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(&segments)?;
    std_symbol_callable_signature(ctx, &registry, symbol)
}

pub fn std_qualified_path_callable_candidate(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
    instantiate_schematics: bool,
) -> Option<CallableCandidate> {
    let segments = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("std") {
        return None;
    }
    let registry = ctx.ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(&segments)?;
    let fact = lower_std_symbol(ctx.ctx, &registry, etas_hir::SymbolId(0), symbol);
    callable_candidate_from_fact(ctx, fact, instantiate_schematics)
}

pub fn std_member_callable_signature_for_symbol(
    ctx: &mut BodyCollectContext<'_, '_>,
    base_symbol: etas_hir::SymbolId,
    member: &str,
) -> Option<CallableSignature> {
    let registry = ctx.ctx.std_registry.clone();
    let base_std_symbol = std_type_symbol_for_hir_symbol(ctx, &registry, base_symbol)?;
    let member_symbol = registry.symbols().find(|symbol| {
        std_symbol_is_type_member_candidate(symbol, member)
            && std_source_method_receiver_matches(symbol, base_std_symbol)
    })?;
    std_symbol_callable_signature(ctx, &registry, member_symbol)
}

pub fn std_member_callable_signature(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: etas_hir::HirExprId,
    member: &str,
) -> Option<CallableSignature> {
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[base] else {
        return None;
    };
    if let ResolveResult::Resolved(symbol) = path.resolution
        && let Some(signature) = std_member_callable_signature_for_symbol(ctx, symbol, member)
    {
        return Some(signature);
    }
    let mut segments = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>();
    if segments.first().copied() != Some("std") {
        return None;
    }
    segments.push(member);
    let registry = ctx.ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(&segments)?;
    std_symbol_callable_signature(ctx, &registry, symbol)
}

fn std_symbol_callable_signature(
    ctx: &mut BodyCollectContext<'_, '_>,
    registry: &StdRegistry,
    symbol: &StdSymbol,
) -> Option<CallableSignature> {
    match lower_std_symbol(ctx.ctx, registry, etas_hir::SymbolId(0), symbol) {
        SymbolTypeFact::Flow { signature }
        | SymbolTypeFact::Agent { signature }
        | SymbolTypeFact::Tool { signature } => Some(signature),
        _ => None,
    }
}

pub fn raw_std_member_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: etas_hir::HirExprId,
    member: &str,
) -> Option<TypeId> {
    if let Some(ty) = std_source_type_member_value_type(ctx, base, member, false) {
        return Some(ty);
    }
    if let HirExpr::Path(path) = &ctx.ctx.hir.exprs[base]
        && let Some(ty) =
            std_qualified_member_value_type_with_instantiation(ctx, path, member, false)
    {
        return Some(ty);
    }
    std_member_value_type(ctx, base, member)
}

pub fn std_declared_field_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: TypeId,
    field: &str,
    span: etas_core::Span,
) -> Option<TypeId> {
    match ctx.ctx.interner.store().get(base).cloned()? {
        Type::Message(inner) => std_declared_message_field_type(ctx, inner, field, span),
        Type::Applied { constructor, args }
            if is_std_message_constructor(ctx, constructor) && args.len() == 1 =>
        {
            std_declared_message_field_type(ctx, args[0], field, span)
        }
        _ => None,
    }
}

fn std_declared_message_field_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    inner: TypeId,
    field: &str,
    span: etas_core::Span,
) -> Option<TypeId> {
    let registry = ctx.ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(&["std", "agent", "message", "Message"])?;
    let StdDecl::Type(decl) = &symbol.decl else {
        return None;
    };
    let StdType::Record(fields) = decl.representation.as_ref()? else {
        return None;
    };
    let field_ty = fields
        .iter()
        .find(|candidate| candidate.name == field)?
        .ty
        .clone();
    let lowered = lower_std_type(ctx.ctx, &registry, &field_ty);
    let substitutions = HashMap::from([("T".to_owned(), inner)]);
    match substitute_named_params(&mut ctx.ctx.interner, lowered, &substitutions) {
        Ok(ty) => Some(ty),
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("standard field type substitution failed: {error}"),
            });
            Some(ctx.primitive(crate::PrimitiveType::Never))
        }
    }
}

fn is_std_message_constructor(
    ctx: &BodyCollectContext<'_, '_>,
    constructor: crate::TypeConstructorId,
) -> bool {
    matches!(
        ctx.ctx.interner.store().get(TypeId(constructor.0)),
        Some(Type::Nominal(nominal)) if nominal.name == "std.agent.message.Message"
    )
}

fn std_source_type_member_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: etas_hir::HirExprId,
    member: &str,
    instantiate_schematics: bool,
) -> Option<TypeId> {
    let HirExpr::Path(path) = &ctx.ctx.hir.exprs[base] else {
        return None;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    std_source_type_member_value_type_for_symbol(ctx, symbol, member, instantiate_schematics)
}

fn std_source_type_member_value_type_for_symbol(
    ctx: &mut BodyCollectContext<'_, '_>,
    base_symbol: etas_hir::SymbolId,
    member: &str,
    instantiate_schematics: bool,
) -> Option<TypeId> {
    let registry = ctx.ctx.std_registry.clone();
    let base_std_symbol = std_type_symbol_for_hir_symbol(ctx, &registry, base_symbol)?;
    let member_symbol = registry.symbols().find(|symbol| {
        std_symbol_is_type_member_candidate(symbol, member)
            && std_source_method_receiver_matches(symbol, base_std_symbol)
    })?;
    let fact = lower_std_symbol(ctx.ctx, &registry, base_symbol, member_symbol);
    if instantiate_schematics {
        value_type_from_fact(ctx, fact)
    } else {
        raw_value_type_from_fact(ctx, fact)
    }
}

fn std_symbol_is_value_method_candidate(symbol: &StdSymbol, method: &str) -> bool {
    let StdDecl::Flow(flow) = &symbol.decl else {
        return false;
    };
    if let Some(source_method) = &flow.source_method {
        return source_method.kind == FlowSourceMethodKind::ValueMethod
            && source_method.name == method;
    }
    symbol.name == method
}

fn std_symbol_is_type_member_candidate(symbol: &StdSymbol, member: &str) -> bool {
    let StdDecl::Flow(flow) = &symbol.decl else {
        return false;
    };
    flow.source_method.as_ref().is_some_and(|source_method| {
        source_method.kind == FlowSourceMethodKind::TypeMember && source_method.name == member
    })
}

fn std_source_method_receiver_matches(symbol: &StdSymbol, receiver_symbol: &StdSymbol) -> bool {
    let StdDecl::Flow(flow) = &symbol.decl else {
        return false;
    };
    let Some(source_method) = &flow.source_method else {
        return false;
    };
    let Some(receiver_name) = std_type_constructor_name(&source_method.receiver) else {
        return false;
    };
    let StdDecl::Type(receiver_decl) = &receiver_symbol.decl else {
        return false;
    };
    receiver_name == receiver_decl.name
}

fn std_type_constructor_name(ty: &StdType) -> Option<&str> {
    match ty {
        StdType::Named(name) => Some(name.as_str()),
        StdType::NamedApplied { name, .. } => Some(name.as_str()),
        StdType::Prompt => Some("Prompt"),
        StdType::PromptPart => Some("PromptPart"),
        _ => None,
    }
}

fn std_type_symbol_for_hir_symbol<'a>(
    ctx: &BodyCollectContext<'_, '_>,
    registry: &'a StdRegistry,
    symbol: etas_hir::SymbolId,
) -> Option<&'a StdSymbol> {
    let symbol = ctx.ctx.hir.symbols.get(symbol)?;
    let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
        return None;
    };
    if *origin != ImportAliasOrigin::StdPrelude
        && path.first().is_none_or(|segment| segment != "std")
    {
        return None;
    }
    let std_symbol = registry.lookup_qualified(path)?;
    matches!(std_symbol.decl, StdDecl::Type(_)).then_some(std_symbol)
}

fn std_member_fact(
    ctx: &mut BodyCollectContext<'_, '_>,
    base_symbol: etas_hir::SymbolId,
    member: &str,
) -> Option<SymbolTypeFact> {
    if member == "run"
        && let Some(SymbolTypeFact::Agent { signature }) = ctx
            .state
            .provisional
            .symbol_types
            .get(&base_symbol)
            .or_else(|| {
                ctx.ctx
                    .symbols
                    .symbol_fact(ctx.ctx.hir, &ctx.ctx.signature_facts, base_symbol)
            })
            .cloned()
    {
        let ty = ctx.ctx.interner.intern(Type::Function(FlowType {
            input: signature.params,
            output: signature.output,
            effects: signature.effects,
        }));
        return Some(SymbolTypeFact::Value { ty });
    }

    let symbol = ctx.ctx.hir.symbols.get(base_symbol)?;
    let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
        return None;
    };
    if *origin != ImportAliasOrigin::StdPrelude
        && path.first().is_none_or(|segment| segment != "std")
    {
        return None;
    }

    let registry = ctx.ctx.std_registry.clone();
    let base_std_symbol = registry.lookup_qualified(path)?;
    if !matches!(base_std_symbol.decl, etas_std::StdDecl::Type(_)) {
        return None;
    }
    let module = registry.module(base_std_symbol.module)?;
    let mut member_path = module.path.clone();
    member_path.push(member.to_owned());
    let member_symbol = registry.lookup_qualified(&member_path)?;
    Some(lower_std_symbol(
        ctx.ctx,
        &registry,
        base_symbol,
        member_symbol,
    ))
}
