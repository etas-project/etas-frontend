use etas_hir_analysis::interprocedural::InterproceduralAnalysisResult;
use etas_utils::ConvergenceStatus;

use crate::EffectPipelineError;
use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};
use crate::facts::{EffectAnalysisOutput, EffectComponentConvergence, EffectConvergenceStatus};
use crate::infer::domain::{EffectSummary, FrontendRejectionReason, InterpreterSupport};
use crate::infer::semantics::semantics::EffectSemantics;
use crate::infer::unit::EffectUnit;

pub fn output_from_result(
    result: InterproceduralAnalysisResult<EffectUnit, EffectSummary, EffectSemantics<'_>>,
) -> Result<EffectAnalysisOutput, EffectPipelineError> {
    let InterproceduralAnalysisResult {
        semantics,
        summaries: summary_store,
        convergence: solver_convergence,
        diagnostics: solver_diagnostics,
        ..
    } = result;
    let hir = semantics.hir;
    let interprocedural_diagnostics = solver_diagnostics
        .into_iter()
        .map(|diagnostic| {
            let anchor = match &diagnostic {
                etas_hir_analysis::interprocedural::InterproceduralDiagnostic::InvalidCondensationGraph => {
                    DiagnosticAnchor::Project
                }
                etas_hir_analysis::interprocedural::InterproceduralDiagnostic::MissingBody { unit } => {
                    DiagnosticAnchor::Unit(*unit)
                }
                etas_hir_analysis::interprocedural::InterproceduralDiagnostic::MissingSummary { call, .. } => {
                    DiagnosticAnchor::Expr(*call)
                }
                etas_hir_analysis::interprocedural::InterproceduralDiagnostic::IterationLimitReached { component } => {
                    component.first().copied().map(DiagnosticAnchor::Unit).unwrap_or(DiagnosticAnchor::Project)
                }
            };
            materialize_effect_diagnostic(
                hir,
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                anchor,
                format!("interprocedural effect analysis failed: {diagnostic:?}"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (_, mut inputs, mut diagnostics, diagnostic_materialization_errors) =
        semantics.into_parts();
    if let Some(error) = diagnostic_materialization_errors.into_iter().next() {
        return Err(error);
    }
    let summaries = summary_store
        .iter()
        .map(|(unit, summary)| (*unit, summary.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (unit, summary) in &summaries {
        inputs
            .unit_effects
            .entry(*unit)
            .or_insert_with(|| summary.clone());
    }
    let unresolved_deferred = inputs
        .deferred_first_class_calls
        .iter()
        .filter(|(unit, _)| !inputs.solved_deferred_units.contains(unit))
        .map(|(unit, calls)| (*unit, calls.clone()))
        .collect::<Vec<_>>();
    for (unit, calls) in unresolved_deferred {
        let summary = inputs.unit_effects.entry(unit).or_default();
        summary.support = InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
        for call in calls {
            diagnostics.push(materialize_effect_diagnostic(
                hir,
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Expr(call),
                "first-class flow call requires a checked latent effect fact",
            )?);
        }
    }
    diagnostics.extend(interprocedural_diagnostics);
    let convergence = solver_convergence
        .into_iter()
        .map(|component| EffectComponentConvergence {
            units: component.units,
            status: match component.status {
                ConvergenceStatus::Converged => EffectConvergenceStatus::Converged,
                ConvergenceStatus::IterationLimitReached => {
                    EffectConvergenceStatus::IterationLimitReached
                }
            },
            iterations: component.stats.iterations,
            changes: component.stats.changes,
        })
        .collect();
    Ok(EffectAnalysisOutput {
        summaries,
        inputs,
        diagnostics,
        convergence,
    })
}
