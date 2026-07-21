use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{
    HirAnnotation, HirAnnotationArg, HirArg, HirExpr, HirExprId, HirFieldInit, HirItem, HirItemId,
    HirLiteral, ResolveResult, SymbolDef, SymbolKind, TopLevelLetClassification,
};
use etas_std::{RequirementSemantics, StdDecl};

use crate::{
    SymbolTypeFact,
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KnownAnnotation {
    Model,
    Tools,
    Limits,
    Derive,
    Test,
    Deprecated,
    Trace,
    Optimization,
}

pub fn validate_annotations(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    let annotations = ctx
        .hir
        .item_annotations
        .iter()
        .flat_map(|(item, annotations)| {
            annotations
                .iter()
                .map(move |annotation| (*item, annotation.clone()))
        })
        .collect::<Vec<_>>();
    for (item, annotation) in annotations {
        validate_annotation(ctx, state, item, &annotation);
    }
}

fn validate_annotation(
    ctx: &mut TypePipelineContext<'_>,
    state: &SignaturePipelineState,
    item: HirItemId,
    annotation: &HirAnnotation,
) {
    let kind = known_annotation(annotation);

    if let Some(kind) = kind {
        validate_annotation_target(ctx, item, annotation, kind);
    }
    validate_static_annotation_args(ctx, annotation, kind);
    match kind {
        Some(KnownAnnotation::Model) => validate_model_annotation(ctx, annotation),
        Some(KnownAnnotation::Tools) => validate_tools_annotation(ctx, state, annotation),
        Some(KnownAnnotation::Derive) => validate_derive_annotation(ctx, annotation),
        Some(KnownAnnotation::Test) => validate_test_annotation(ctx, annotation),
        Some(KnownAnnotation::Deprecated) => validate_deprecated_annotation(ctx, annotation),
        Some(KnownAnnotation::Optimization) => validate_optimization_annotation(ctx, annotation),
        _ => {}
    }
}

fn known_annotation(annotation: &HirAnnotation) -> Option<KnownAnnotation> {
    match annotation_name(annotation).as_str() {
        "model" => Some(KnownAnnotation::Model),
        "tools" => Some(KnownAnnotation::Tools),
        "limits" => Some(KnownAnnotation::Limits),
        "derive" => Some(KnownAnnotation::Derive),
        "test" => Some(KnownAnnotation::Test),
        "deprecated" => Some(KnownAnnotation::Deprecated),
        "trace" => Some(KnownAnnotation::Trace),
        "optimization" => Some(KnownAnnotation::Optimization),
        _ => None,
    }
}

fn annotation_name(annotation: &HirAnnotation) -> String {
    annotation
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn validate_annotation_target(
    ctx: &mut TypePipelineContext<'_>,
    item: HirItemId,
    annotation: &HirAnnotation,
    kind: KnownAnnotation,
) {
    let Some(hir_item) = ctx.hir.items.get(item) else {
        return;
    };
    let allowed = match kind {
        KnownAnnotation::Model | KnownAnnotation::Tools => matches!(hir_item, HirItem::Agent(_)),
        KnownAnnotation::Limits | KnownAnnotation::Trace | KnownAnnotation::Optimization => {
            matches!(
                hir_item,
                HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_)
            )
        }
        KnownAnnotation::Derive => matches!(hir_item, HirItem::Type(_) | HirItem::Enum(_)),
        KnownAnnotation::Test => matches!(hir_item, HirItem::Flow(_)),
        KnownAnnotation::Deprecated => !matches!(hir_item, HirItem::Error { .. }),
    };
    if !allowed {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::InvalidAnnotation,
            annotation.span,
            format!(
                "annotation `@{}` cannot be applied to this item",
                annotation_name(annotation)
            ),
        ));
    }
}

fn validate_static_annotation_args(
    ctx: &mut TypePipelineContext<'_>,
    annotation: &HirAnnotation,
    kind: Option<KnownAnnotation>,
) {
    for arg in &annotation.args {
        let (value, span) = match arg {
            HirAnnotationArg::Positional { value, span } => (*value, *span),
            HirAnnotationArg::Named { value, span, .. } => (*value, *span),
        };
        if !is_static_annotation_expr(ctx, value, kind) {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidAnnotation,
                span,
                "annotation argument must be a static literal, path, array, record, or compile-time const",
            ));
        }
    }
}

fn is_static_annotation_expr(
    ctx: &TypePipelineContext<'_>,
    expr: HirExprId,
    annotation: Option<KnownAnnotation>,
) -> bool {
    let Some(expr) = ctx.hir.exprs.get(expr) else {
        return false;
    };
    match expr {
        HirExpr::Literal(literal) => is_static_literal(literal),
        HirExpr::Path(path) => is_static_annotation_path(ctx, path.resolution.clone()),
        HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } | HirExpr::Set { elems, .. } => {
            elems
                .iter()
                .copied()
                .all(|elem| is_static_annotation_expr(ctx, elem, annotation))
        }
        HirExpr::Tuple { elems, .. } => elems
            .iter()
            .copied()
            .all(|elem| is_static_annotation_expr(ctx, elem, annotation)),
        HirExpr::Record(record) => {
            record.path.is_none()
                && record.generic_args.is_empty()
                && record.fields.iter().all(|field| match field {
                    HirFieldInit::Named { value, .. } => {
                        is_static_annotation_expr(ctx, *value, annotation)
                    }
                    HirFieldInit::Shorthand { resolution, .. } => match resolution {
                        ResolveResult::Resolved(symbol) => is_static_symbol(ctx, *symbol),
                        _ => false,
                    },
                })
        }
        HirExpr::Call {
            callee,
            generic_args,
            args,
            ..
        } if annotation == Some(KnownAnnotation::Limits) => {
            generic_args.is_empty()
                && is_known_limit_constructor(ctx, *callee)
                && args.iter().all(|arg| match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => {
                        is_static_annotation_expr(ctx, *value, annotation)
                    }
                })
        }
        HirExpr::Call {
            callee,
            generic_args,
            args,
            ..
        } if annotation == Some(KnownAnnotation::Trace) => {
            generic_args.is_empty()
                && is_known_trace_constructor(ctx, *callee)
                && args.iter().all(|arg| match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => {
                        is_static_annotation_expr(ctx, *value, annotation)
                    }
                })
        }
        HirExpr::EmptyRecordOrMap { .. } | HirExpr::EmptySequence { .. } => true,
        _ => false,
    }
}

fn is_static_literal(literal: &HirLiteral) -> bool {
    matches!(
        literal,
        HirLiteral::Bool { .. }
            | HirLiteral::Int { .. }
            | HirLiteral::Float { .. }
            | HirLiteral::String { .. }
            | HirLiteral::Char { .. }
    )
}

fn is_static_annotation_path(ctx: &TypePipelineContext<'_>, resolution: ResolveResult) -> bool {
    match resolution {
        ResolveResult::Resolved(symbol) => is_static_symbol(ctx, symbol),
        ResolveResult::PartiallyResolved(_)
        | ResolveResult::Unresolved
        | ResolveResult::Ambiguous(_) => false,
    }
}

fn is_static_symbol(ctx: &TypePipelineContext<'_>, symbol: etas_hir::SymbolId) -> bool {
    let Some(symbol) = ctx.hir.symbols.get(symbol) else {
        return false;
    };
    match &symbol.def {
        SymbolDef::TopLevelLet { classification, .. } => {
            matches!(classification, TopLevelLetClassification::Const)
        }
        SymbolDef::Item { .. }
        | SymbolDef::EnumVariant { .. }
        | SymbolDef::TypeParam { .. }
        | SymbolDef::EffectParam { .. }
        | SymbolDef::ImportAlias { .. }
        | SymbolDef::Synthetic { .. } => true,
        SymbolDef::Module { .. } => true,
        SymbolDef::Param { .. }
        | SymbolDef::Local { .. }
        | SymbolDef::PatternBinding { .. }
        | SymbolDef::Field { .. }
        | SymbolDef::EffectAction { .. }
        | SymbolDef::Error => false,
    }
}

fn is_known_limit_constructor(ctx: &TypePipelineContext<'_>, callee: HirExprId) -> bool {
    let Some(HirExpr::Path(path)) = ctx.hir.exprs.get(callee) else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = ctx.hir.symbols.get(symbol) else {
        return false;
    };
    let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
        return false;
    };
    let registry = etas_std::standard_registry();
    let Some(std_symbol) = registry.lookup_qualified(path) else {
        return false;
    };
    let StdDecl::Requirement(requirement) = &std_symbol.decl else {
        return false;
    };
    matches!(requirement.semantics, RequirementSemantics::Limit(_))
}

fn is_known_trace_constructor(ctx: &TypePipelineContext<'_>, callee: HirExprId) -> bool {
    let Some(HirExpr::Path(path)) = ctx.hir.exprs.get(callee) else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = ctx.hir.symbols.get(symbol) else {
        return false;
    };
    let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
        return false;
    };
    let registry = etas_std::standard_registry();
    let Some(std_symbol) = registry.lookup_qualified(path) else {
        return false;
    };
    if std_symbol.qualified_path.as_slice() != ["std", "runtime", "trace", std_symbol.name.as_str()]
    {
        return false;
    }
    let StdDecl::Flow(flow) = &std_symbol.decl else {
        return false;
    };
    flow.public_effects.is_empty() && flow.requested_actions.is_empty()
}

fn validate_model_annotation(ctx: &mut TypePipelineContext<'_>, annotation: &HirAnnotation) {
    let mut positional_count = 0usize;
    for arg in &annotation.args {
        match arg {
            HirAnnotationArg::Positional { value, span } => {
                positional_count += 1;
                if positional_count > 1 {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidAnnotation,
                        *span,
                        "`@model` accepts at most one positional model name",
                    ));
                }
                if !is_string_literal_expr(ctx, *value) {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidAnnotation,
                        *span,
                        "`@model` positional argument must be a string literal model name",
                    ));
                }
            }
            HirAnnotationArg::Named {
                name, value, span, ..
            } => {
                if !matches!(name.as_str(), "adapter" | "provider" | "model") {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidAnnotation,
                        *span,
                        format!("unsupported `@model` argument `{name}`"),
                    ));
                    continue;
                }
                if !is_string_literal_expr(ctx, *value) {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidAnnotation,
                        *span,
                        format!("`@model` argument `{name}` must be a string literal"),
                    ));
                }
            }
        }
    }
}

fn is_string_literal_expr(ctx: &TypePipelineContext<'_>, expr: HirExprId) -> bool {
    matches!(
        ctx.hir.exprs.get(expr),
        Some(HirExpr::Literal(HirLiteral::String { .. }))
    )
}

fn validate_test_annotation(ctx: &mut TypePipelineContext<'_>, annotation: &HirAnnotation) {
    if annotation.args.is_empty() {
        return;
    }
    ctx.diagnostics.push(Diagnostic::type_check(
        TypeDiagnosticCode::InvalidAnnotation,
        annotation.span,
        "`@test` does not accept arguments",
    ));
}

fn validate_deprecated_annotation(ctx: &mut TypePipelineContext<'_>, annotation: &HirAnnotation) {
    if annotation.args.len() > 1 {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::InvalidAnnotation,
            annotation.span,
            "`@deprecated` accepts at most one message string",
        ));
    }
    for arg in &annotation.args {
        match arg {
            HirAnnotationArg::Positional { value, span } => {
                if !is_string_literal_expr(ctx, *value) {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidAnnotation,
                        *span,
                        "`@deprecated` message must be a string literal",
                    ));
                }
            }
            HirAnnotationArg::Named { span, .. } => {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidAnnotation,
                    *span,
                    "`@deprecated` accepts only an optional positional message string",
                ));
            }
        }
    }
}

fn validate_optimization_annotation(ctx: &mut TypePipelineContext<'_>, annotation: &HirAnnotation) {
    for arg in &annotation.args {
        let value = match arg {
            HirAnnotationArg::Positional { value, .. } | HirAnnotationArg::Named { value, .. } => {
                *value
            }
        };
        for marker_expr in annotation_sequence_entries(ctx, value) {
            if !is_static_path_expr(ctx, marker_expr) {
                let span = ctx
                    .hir
                    .exprs
                    .get(marker_expr)
                    .map(|expr| expr.span(&ctx.hir.blocks))
                    .unwrap_or(annotation.span);
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidAnnotation,
                    span,
                    "`@optimization` entries must be static path markers",
                ));
            }
        }
    }
}

fn is_static_path_expr(ctx: &TypePipelineContext<'_>, expr: HirExprId) -> bool {
    let Some(HirExpr::Path(path)) = ctx.hir.exprs.get(expr) else {
        return false;
    };
    is_static_annotation_path(ctx, path.resolution.clone())
}

fn validate_tools_annotation(
    ctx: &mut TypePipelineContext<'_>,
    state: &SignaturePipelineState,
    annotation: &HirAnnotation,
) {
    for arg in &annotation.args {
        let value = match arg {
            HirAnnotationArg::Positional { value, .. } => *value,
            HirAnnotationArg::Named {
                name, value, span, ..
            } if name == "choice" || name == "mode" => {
                if !matches!(
                    ctx.hir.exprs.get(*value),
                    Some(HirExpr::Literal(HirLiteral::String { value, .. }))
                        if value == "auto" || value == "required"
                ) {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::InvalidAnnotation,
                        *span,
                        "`@tools` choice must be `auto` or `required`",
                    ));
                }
                continue;
            }
            HirAnnotationArg::Named { name, value, .. } if name == "tools" || name == "items" => {
                *value
            }
            HirAnnotationArg::Named { name, span, .. } => {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidAnnotation,
                    *span,
                    format!("unsupported `@tools` argument `{name}`"),
                ));
                continue;
            }
        };
        for tool_expr in annotation_tool_entries(ctx, value) {
            if !is_tool_reference(ctx, state, tool_expr) {
                let span = ctx
                    .hir
                    .exprs
                    .get(tool_expr)
                    .map(|expr| expr.span(&ctx.hir.blocks))
                    .unwrap_or(annotation.span);
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidAnnotation,
                    span,
                    "`@tools` entries must reference tool declarations or imported tool symbols",
                ));
            }
        }
    }
}

fn validate_derive_annotation(ctx: &mut TypePipelineContext<'_>, annotation: &HirAnnotation) {
    for arg in &annotation.args {
        let value = match arg {
            HirAnnotationArg::Positional { value, .. } | HirAnnotationArg::Named { value, .. } => {
                *value
            }
        };
        for capability_expr in annotation_sequence_entries(ctx, value) {
            if !is_derivable_capability(ctx, capability_expr) {
                let span = ctx
                    .hir
                    .exprs
                    .get(capability_expr)
                    .map(|expr| expr.span(&ctx.hir.blocks))
                    .unwrap_or(annotation.span);
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidAnnotation,
                    span,
                    "`@derive` entries must reference compiler-derivable std capabilities",
                ));
            }
        }
    }
}

fn annotation_tool_entries(ctx: &TypePipelineContext<'_>, expr: HirExprId) -> Vec<HirExprId> {
    annotation_sequence_entries(ctx, expr)
}

fn annotation_sequence_entries(ctx: &TypePipelineContext<'_>, expr: HirExprId) -> Vec<HirExprId> {
    match ctx.hir.exprs.get(expr) {
        Some(HirExpr::Array { elems, .. }) | Some(HirExpr::List { elems, .. }) => elems.clone(),
        _ => vec![expr],
    }
}

fn is_tool_reference(
    ctx: &TypePipelineContext<'_>,
    state: &SignaturePipelineState,
    expr: HirExprId,
) -> bool {
    let Some(HirExpr::Path(path)) = ctx.hir.exprs.get(expr) else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = ctx.hir.symbols.get(symbol) else {
        return false;
    };
    if symbol.kind == SymbolKind::Tool {
        return true;
    }
    if ctx
        .symbols
        .symbol_fact(ctx.hir, &ctx.signature_facts, symbol.id)
        .or_else(|| {
            ctx.symbols
                .canonical_symbol(ctx.hir, symbol.id)
                .and_then(|canonical| state.symbol_types.get(&canonical))
        })
        .or_else(|| state.symbol_types.get(&symbol.id))
        .is_some_and(|fact| matches!(fact, SymbolTypeFact::Tool { .. }))
    {
        return true;
    }
    match symbol.def {
        SymbolDef::Item { item } => matches!(ctx.hir.items.get(item), Some(HirItem::Tool(_))),
        _ => false,
    }
}

fn is_derivable_capability(ctx: &TypePipelineContext<'_>, expr: HirExprId) -> bool {
    let Some(HirExpr::Path(path)) = ctx.hir.exprs.get(expr) else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = ctx.hir.symbols.get(symbol) else {
        return false;
    };
    let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
        return false;
    };
    let registry = etas_std::standard_registry();
    let Some(std_symbol) = registry.lookup_qualified(path) else {
        return false;
    };
    let StdDecl::Type(decl) = &std_symbol.decl else {
        return false;
    };
    decl.derivable
}
