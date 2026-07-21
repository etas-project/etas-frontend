use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{HirExprId, HirProgram};

use crate::{EffectPipelineError, EffectUnit};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiagnosticAnchor {
    Expr(HirExprId),
    Unit(EffectUnit),
    Project,
}

pub(crate) fn materialize_effect_diagnostic(
    hir: &HirProgram,
    code: EffectDiagnosticCode,
    anchor: DiagnosticAnchor,
    message: impl Into<String>,
) -> Result<Diagnostic, EffectPipelineError> {
    let span = resolve_anchor(hir, &anchor).ok_or_else(|| {
        EffectPipelineError::MissingDiagnosticAnchor {
            artifact: format!("{anchor:?}"),
        }
    })?;
    Ok(Diagnostic::effect_check(code, span, message))
}

pub(crate) fn resolve_anchor(hir: &HirProgram, anchor: &DiagnosticAnchor) -> Option<Span> {
    match anchor {
        DiagnosticAnchor::Expr(expr) => hir.exprs.get(*expr).map(|expr| expr.span(&hir.blocks)),
        DiagnosticAnchor::Unit(unit) => unit_span(hir, *unit),
        DiagnosticAnchor::Project => project_span(hir),
    }
}

pub(crate) fn project_span(hir: &HirProgram) -> Option<Span> {
    hir.modules_arena
        .iter()
        .next()
        .map(|(_, module)| module.span)
        .or_else(|| hir.items.iter().next().map(|(_, item)| item.span()))
        .or_else(|| {
            hir.symbols
                .iter()
                .next()
                .map(|symbol| symbol.definition_span)
        })
}

fn unit_span(hir: &HirProgram, unit: EffectUnit) -> Option<Span> {
    match unit {
        EffectUnit::Item(item) => hir.items.get(item).map(etas_hir::HirItem::span),
        EffectUnit::HandlerArm { arm, .. } => hir.handler_arms.get(arm).map(|arm| arm.span),
        EffectUnit::AnonymousFlow { value, .. }
        | EffectUnit::FirstClassFlowCall { call: value, .. } => {
            hir.exprs.get(value).map(|expr| expr.span(&hir.blocks))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_anchor_requires_real_hir_source() {
        assert_eq!(
            resolve_anchor(&HirProgram::default(), &DiagnosticAnchor::Project),
            None
        );
    }
}
