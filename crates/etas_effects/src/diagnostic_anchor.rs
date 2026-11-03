use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{HirExprId, HirProgram, HirStmt, HirStmtId, SymbolId};

use crate::{EffectPipelineError, EffectUnit};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DiagnosticAnchor {
    Expr(HirExprId),
    Stmt(HirStmtId),
    Symbol(SymbolId),
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
        DiagnosticAnchor::Stmt(stmt) => match hir.stmts.get(*stmt)? {
            HirStmt::If(expr) | HirStmt::Match(expr) => {
                hir.exprs.get(*expr).map(|expr| expr.span(&hir.blocks))
            }
            HirStmt::Let { span, .. }
            | HirStmt::Var { span, .. }
            | HirStmt::Assign { span, .. }
            | HirStmt::For { span, .. }
            | HirStmt::While { span, .. }
            | HirStmt::Retry { span, .. }
            | HirStmt::Resume { span, .. }
            | HirStmt::Finish { span, .. }
            | HirStmt::Return { span, .. }
            | HirStmt::Break { span }
            | HirStmt::Continue { span }
            | HirStmt::Expr { span, .. }
            | HirStmt::Error { span } => Some(*span),
        },
        DiagnosticAnchor::Symbol(symbol) => {
            hir.symbols.get(*symbol).map(|data| data.definition_span)
        }
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
        EffectUnit::AnonymousFlow { value, .. } => {
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
