use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirDeclarationConformanceTarget, HirItem, ResolveResult, SymbolKind};

use crate::{
    CallableSpecSatisfactionFact, SpecKind, SpecSignature, TraceSpecConformanceFact,
    TraceSpecConformanceTarget,
    lower::type_ref::lower_type_ref,
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn validate_flow_spec_satisfaction(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    validate_type_param_bounds(ctx, state);
    validate_flow_specs(ctx, state);
    validate_conformance_target_kinds(ctx, state);
    collect_declaration_spec_satisfactions(ctx, state);
}

fn validate_type_param_bounds(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    for bounds in state.type_param_bounds.values() {
        for bound in bounds {
            let Some(signature) = spec_signature(ctx, state, bound.spec_symbol) else {
                let Some(symbol) = ctx.hir.symbols.get(bound.param) else {
                    continue;
                };
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidImplTargetKind,
                    symbol.definition_span,
                    "type parameter bound must resolve to a spec",
                ));
                continue;
            };
            let expected_args = signature.params.len();
            let spec_name = signature.name.clone();
            let spec_kind = signature.kind;
            let is_type_spec = matches!(spec_kind, SpecKind::TypeSpec);
            if bound.args.len() != expected_args {
                let Some(symbol) = ctx.hir.symbols.get(bound.param) else {
                    continue;
                };
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::ArityMismatch,
                    symbol.definition_span,
                    format!(
                        "spec `{}` expects {} type argument(s), got {}",
                        spec_name,
                        expected_args,
                        bound.args.len()
                    ),
                ));
            }
            if !is_type_spec {
                let Some(symbol) = ctx.hir.symbols.get(bound.param) else {
                    continue;
                };
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::TypeMismatch,
                    symbol.definition_span,
                    format!(
                        "{} spec cannot be used as a type parameter bound",
                        spec_kind_name(spec_kind)
                    ),
                ));
            }
        }
    }
}

fn validate_flow_specs(ctx: &mut TypePipelineContext<'_>, state: &SignaturePipelineState) {
    let callable_conformances = ctx
        .hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(("flow", flow.conformances.clone())),
            HirItem::Tool(tool) => Some(("tool", tool.conformances.clone())),
            HirItem::Agent(agent) => Some(("agent", agent.conformances.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (callable_kind, conformances) in callable_conformances {
        for conformance in conformances {
            let spec_ref = match conformance.target {
                HirDeclarationConformanceTarget::Path(spec_ref) => spec_ref,
                HirDeclarationConformanceTarget::InlineTraceSpec(_) => continue,
                HirDeclarationConformanceTarget::Error { .. } => continue,
            };
            let ResolveResult::Resolved(spec_symbol) = spec_ref.spec_path.resolution else {
                continue;
            };
            let Some(signature) = spec_signature(ctx, state, spec_symbol) else {
                continue;
            };
            if matches!(signature.kind, SpecKind::TypeSpec) {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidImplTargetKind,
                    spec_ref.span,
                    format!(
                        "type spec cannot be used as {} {callable_kind} spec",
                        article(callable_kind)
                    ),
                ));
            }
        }
    }
}

fn collect_declaration_spec_satisfactions(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let callable_conformances = ctx
        .hir
        .items
        .iter()
        .filter_map(|(item, hir_item)| match hir_item {
            HirItem::Flow(flow) => Some((item, flow.symbol, flow.conformances.clone())),
            HirItem::Tool(tool) => Some((item, tool.symbol, tool.conformances.clone())),
            HirItem::Agent(agent) => Some((item, agent.symbol, agent.conformances.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();

    for (item, symbol, conformances) in callable_conformances {
        for conformance in conformances {
            match conformance.target {
                HirDeclarationConformanceTarget::Path(spec_ref) => {
                    let ResolveResult::Resolved(spec_symbol) = spec_ref.spec_path.resolution else {
                        continue;
                    };
                    let spec_symbol = ctx
                        .symbols
                        .canonical_symbol(ctx.hir, spec_symbol)
                        .unwrap_or(spec_symbol);
                    let Some(signature) = spec_signature(ctx, state, spec_symbol) else {
                        continue;
                    };
                    let spec_kind = signature.kind;
                    let args = spec_ref
                        .spec_args
                        .iter()
                        .filter_map(|arg| lower_type_ref(ctx, *arg))
                        .collect::<Vec<_>>();
                    match spec_kind {
                        SpecKind::CallableSpec => {
                            state
                                .callable_spec_satisfactions
                                .push(CallableSpecSatisfactionFact {
                                    item,
                                    symbol,
                                    spec_symbol,
                                    args,
                                    span: spec_ref.span,
                                });
                        }
                        SpecKind::TraceSpec => {
                            state
                                .trace_spec_conformances
                                .push(TraceSpecConformanceFact {
                                    item,
                                    symbol,
                                    target: TraceSpecConformanceTarget::Named { spec_symbol, args },
                                    span: spec_ref.span,
                                });
                        }
                        SpecKind::TypeSpec => {}
                    }
                }
                HirDeclarationConformanceTarget::InlineTraceSpec(expr) => {
                    state
                        .trace_spec_conformances
                        .push(TraceSpecConformanceFact {
                            item,
                            symbol,
                            target: TraceSpecConformanceTarget::Inline,
                            span: expr.span(),
                        });
                }
                HirDeclarationConformanceTarget::Error { .. } => {}
            }
        }
    }
}

fn spec_kind_name(kind: SpecKind) -> &'static str {
    match kind {
        SpecKind::TypeSpec => "type",
        SpecKind::CallableSpec => "callable",
        SpecKind::TraceSpec => "trace",
    }
}

fn article(kind: &str) -> &'static str {
    match kind {
        "agent" => "an",
        _ => "a",
    }
}

fn validate_conformance_target_kinds(
    ctx: &mut TypePipelineContext<'_>,
    state: &SignaturePipelineState,
) {
    let conformances = ctx
        .hir
        .items
        .iter()
        .filter_map(|(_, item)| match item {
            HirItem::Flow(flow) => Some(flow.conformances.clone()),
            HirItem::Tool(tool) => Some(tool.conformances.clone()),
            HirItem::Agent(agent) => Some(agent.conformances.clone()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    for conformance in conformances {
        let spec_ref = match conformance.target {
            HirDeclarationConformanceTarget::Path(spec_ref) => spec_ref,
            HirDeclarationConformanceTarget::InlineTraceSpec(_) => continue,
            HirDeclarationConformanceTarget::Error { .. } => continue,
        };
        let ResolveResult::Resolved(symbol) = spec_ref.spec_path.resolution else {
            continue;
        };
        if spec_signature(ctx, state, symbol).is_some() {
            continue;
        }
        let Some(symbol_data) = ctx.hir.symbols.get(symbol) else {
            continue;
        };
        if matches!(symbol_data.kind, SymbolKind::Protocol) {
            continue;
        }
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::InvalidImplTargetKind,
            spec_ref.span,
            "declaration conformance target must resolve to a spec or protocol",
        ));
    }
}

fn spec_signature<'a>(
    ctx: &'a TypePipelineContext<'_>,
    state: &'a SignaturePipelineState,
    symbol: etas_hir::SymbolId,
) -> Option<&'a SpecSignature> {
    state
        .spec_signatures
        .get(&symbol)
        .or_else(|| ctx.signature_facts.spec_signatures.get(&symbol))
}
