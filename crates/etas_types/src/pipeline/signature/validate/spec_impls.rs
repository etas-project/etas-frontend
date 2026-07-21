use std::collections::{HashMap, HashSet};

use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirImplItem, HirImplTarget, HirItem, ResolveResult, SymbolKind};

use crate::pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState};

pub fn validate_spec_impls(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    validate_impl_member_kinds(ctx);
    validate_duplicate_impls(ctx, state);
    validate_method_bearing_spec_groups(ctx, state);
    validate_required_methods(ctx, state);
}

fn validate_impl_member_kinds(ctx: &mut TypePipelineContext<'_>) {
    for (_, item) in ctx.hir.items.iter() {
        let HirItem::Impl(decl) = item else {
            continue;
        };
        match &decl.target {
            HirImplTarget::Inherent { target, .. } => {
                let is_effect_impl = matches!(
                    target.resolution,
                    ResolveResult::Resolved(symbol)
                        if ctx
                            .hir
                            .symbols
                            .get(symbol)
                            .is_some_and(|symbol| symbol.kind == SymbolKind::Effect)
                );
                for item in &decl.items {
                    match item {
                        HirImplItem::Action(action) if !is_effect_impl => {
                            ctx.diagnostics.push(Diagnostic::type_check(
                                TypeDiagnosticCode::InvalidImplMemberKind,
                                action.span,
                                "only flow methods are valid inside type impls",
                            ));
                        }
                        HirImplItem::Flow(flow) if is_effect_impl => {
                            ctx.diagnostics.push(Diagnostic::type_check(
                                TypeDiagnosticCode::InvalidImplMemberKind,
                                flow.span,
                                "effect impls may only declare effect actions",
                            ));
                        }
                        _ => {}
                    }
                }
            }
            HirImplTarget::SpecSatisfaction { .. } => {
                let has_flow = decl
                    .items
                    .iter()
                    .any(|item| matches!(item, HirImplItem::Flow(_)));
                let has_action = decl
                    .items
                    .iter()
                    .any(|item| matches!(item, HirImplItem::Action(_)));
                if has_flow && has_action {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidImplMemberKind,
                        decl.span,
                        "spec impl cannot mix flow methods and effect actions",
                    ));
                }
            }
            HirImplTarget::Error => {}
        }
    }
}

fn validate_duplicate_impls(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    let mut seen = HashSet::new();
    for impl_fact in &state.spec_impls {
        let key = (
            impl_fact.self_type,
            impl_fact.spec_symbol,
            impl_fact.args.clone(),
        );
        if !seen.insert(key) {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplTargetKind,
                impl_fact.span,
                format!(
                    "duplicate impl of spec `{}` for this type",
                    symbol_name(ctx, impl_fact.spec_symbol)
                ),
            ));
        }
    }
}

fn validate_method_bearing_spec_groups(
    ctx: &mut TypePipelineContext<'_>,
    state: &SignaturePipelineState,
) {
    let mut group_counts = HashMap::<u32, usize>::new();
    let mut group_spans = HashMap::<u32, etas_core::Span>::new();
    for impl_fact in &state.spec_impls {
        let method_count = state
            .spec_signatures
            .get(&impl_fact.spec_symbol)
            .map(|signature| signature.methods.len())
            .unwrap_or_default();
        if method_count > 0 {
            *group_counts.entry(impl_fact.impl_group).or_default() += 1;
            group_spans
                .entry(impl_fact.impl_group)
                .or_insert(impl_fact.span);
        }
    }
    for (group, count) in group_counts {
        if count > 1 {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplMemberKind,
                group_spans[&group],
                "multi-spec impl may include at most one method-bearing spec",
            ));
        }
    }
}

fn validate_required_methods(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    for impl_fact in &state.spec_impls {
        let Some(signature) = state.spec_signatures.get(&impl_fact.spec_symbol) else {
            continue;
        };
        let required = signature
            .methods
            .iter()
            .map(|method| method.name.clone())
            .collect::<HashSet<_>>();
        let provided = impl_fact
            .methods
            .iter()
            .map(|method| method.name.clone())
            .collect::<HashSet<_>>();
        if !required.is_empty() && provided.is_empty() {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplMemberKind,
                impl_fact.span,
                format!(
                    "bodyless impl of spec `{}` is missing required method `{}`",
                    symbol_name(ctx, impl_fact.spec_symbol),
                    required
                        .iter()
                        .next()
                        .expect("non-empty required method set")
                ),
            ));
            continue;
        }
        for method in required.difference(&provided) {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplMemberKind,
                impl_fact.span,
                format!("missing required method `{method}`"),
            ));
        }
        for method in provided.difference(&required) {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidImplMemberKind,
                impl_fact.span,
                format!(
                    "method `{method}` is not declared by spec `{}`",
                    symbol_name(ctx, impl_fact.spec_symbol)
                ),
            ));
        }
    }
}

fn symbol_name(ctx: &TypePipelineContext<'_>, symbol: etas_hir::SymbolId) -> String {
    let name = ctx
        .hir
        .symbols
        .get(symbol)
        .map(|symbol| symbol.name.clone())
        .unwrap_or_else(|| format!("symbol{}", symbol.0));
    name.rsplit('.').next().unwrap_or(&name).to_owned()
}
