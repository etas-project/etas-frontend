use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirArg, HirExpr, HirItem, ResolveResult, SymbolDef, SymbolKind};

use crate::pipeline::{body::state::BodyPipelineState, context::TypePipelineContext};

pub(super) fn validate(ctx: &mut TypePipelineContext<'_>, state: &BodyPipelineState) {
    let selected = state
        .solver_report
        .std_intrinsic_calls
        .iter()
        .filter(|(_, call)| call.intrinsic == etas_std::intrinsic::runtime::TOOL_REFERENCE)
        .map(|(expr, _)| expr)
        .chain(
            state
                .solver_report
                .checked_method_calls
                .iter()
                .filter_map(|(expr, call)| {
                    matches!(
                        call.target,
                        crate::method::MethodTarget::Standard {
                            intrinsic: Some(etas_std::intrinsic::runtime::TOOL_REFERENCE),
                            ..
                        }
                    )
                    .then_some(expr)
                }),
        );
    for expr in selected {
        let Some(HirExpr::Call { args, span, .. } | HirExpr::MethodCall { args, span, .. }) =
            ctx.hir.exprs.get(*expr)
        else {
            continue;
        };
        let valid = match args.as_slice() {
            [HirArg::Positional(value) | HirArg::Named { value, .. }] => {
                match ctx.hir.exprs.get(*value) {
                    Some(HirExpr::Path(path)) => match path.resolution {
                        ResolveResult::Resolved(symbol) => ctx
                            .symbols
                            .canonical_symbol(ctx.hir, symbol)
                            .and_then(|symbol| ctx.hir.symbols.get(symbol))
                            .is_some_and(|symbol| match symbol.def {
                                SymbolDef::Item { item } => {
                                    symbol.kind == SymbolKind::Tool
                                        && matches!(ctx.hir.items.get(item), Some(HirItem::Tool(_)))
                                }
                                SymbolDef::ImportAlias { .. } => matches!(
                                    ctx.signature_base
                                        .map_or(&ctx.signature_facts, |base| &base.facts)
                                        .symbol_types
                                        .get(&symbol.id),
                                    Some(crate::SymbolTypeFact::Tool { .. })
                                ),
                                _ => false,
                            }),
                        _ => false,
                    },
                    _ => false,
                }
            }
            _ => false,
        };
        if !valid {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                *span,
                "tool.reference requires a resolved native tool declaration",
            ));
        }
    }
}
