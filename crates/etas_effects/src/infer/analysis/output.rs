use etas_hir_analysis::interprocedural::SummarySolverResult;
use etas_utils::ConvergenceStatus;

use crate::EffectPipelineError;
use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};
use crate::facts::{EffectAnalysisOutput, EffectComponentConvergence, EffectConvergenceStatus};
use crate::infer::domain::{EffectSummary, FrontendRejectionReason, InterpreterSupport};
use crate::infer::semantics::engine::EffectSemantics;
use crate::infer::unit::EffectUnit;

pub fn output_from_result(
    mut semantics: EffectSemantics<'_>,
    result: SummarySolverResult<EffectUnit, EffectSummary>,
) -> Result<EffectAnalysisOutput, EffectPipelineError> {
    let SummarySolverResult {
        summaries: summary_store,
        convergence: solver_convergence,
        diagnostics: solver_diagnostics,
        ..
    } = result;
    for (unit, summary) in summary_store.iter() {
        semantics.inputs.unit_effects.insert(*unit, summary.clone());
    }
    semantics.materialize_captured_callable_analyses()?;
    semantics.materialize_metadata_callable_analyses()?;
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
    let (mut inputs, mut diagnostics) = semantics.into_parts()?;
    let summaries = summary_store.into_summaries();
    let unresolved_deferred = inputs
        .deferred_first_class_calls
        .iter()
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
    for (unit, calls) in &inputs.unresolved_parameter_calls {
        inputs.unit_effects.entry(*unit).or_default().support =
            InterpreterSupport::Rejected(FrontendRejectionReason::EscapedEffect);
        for call in calls {
            diagnostics.push(materialize_effect_diagnostic(
                hir,
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                DiagnosticAnchor::Expr(*call),
                "first-class flow call requires a checked latent effect fact; an escaping row does not describe requested actions",
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
