use etas_hir_analysis::interprocedural::{
    InterproceduralAnalysis, InterproceduralAnalysisResult, SummaryStore,
};

use crate::{EffectOutput, EffectPipelineError, EffectRegistry, EffectUnit};

use super::model::TraceSpecModelStore;
use super::semantics::TraceSpecSemantics;
use super::validate::reject_item_for_trace_spec;
use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct TraceSpecAnalysisOutput {
    pub summaries: SummaryStore<EffectUnit, super::domain::TraceSpecSummary>,
}

pub fn analyze_trace_spec_monitors(
    hir: &etas_hir::HirRevision,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    units: &[EffectUnit],
) -> Result<TraceSpecAnalysisOutput, EffectPipelineError> {
    analyze_with_control(hir, types, registry, effects, models, units, None)
}

pub fn analyze_trace_spec_monitors_controlled(
    hir: &etas_hir::HirRevision,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    units: &[EffectUnit],
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<TraceSpecAnalysisOutput, EffectPipelineError> {
    analyze_with_control(hir, types, registry, effects, models, units, Some(control))
}

fn analyze_with_control(
    hir: &etas_hir::HirRevision,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    effects: &mut EffectOutput,
    models: &TraceSpecModelStore,
    units: &[EffectUnit],
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<TraceSpecAnalysisOutput, EffectPipelineError> {
    let trace_spec_items = models.referenced_by_item.keys().copied().collect();
    let analysis = InterproceduralAnalysis::new(
        units.iter().copied(),
        TraceSpecSemantics::new(hir, types, registry, &effects.facts, trace_spec_items),
    );
    let result = match control {
        Some(control) => analysis
            .solve_controlled(control)
            .map_err(EffectPipelineError::Interrupted)?,
        None => analysis.solve(),
    };
    let InterproceduralAnalysisResult {
        semantics,
        summaries,
        convergence,
        diagnostics: analysis_diagnostics,
        ..
    } = result;
    let (diagnostics, rejected_items, diagnostic_materialization_errors) = semantics.into_parts();
    if let Some(error) = diagnostic_materialization_errors.into_iter().next() {
        return Err(error);
    }
    effects.diagnostics.extend(diagnostics);
    for diagnostic in analysis_diagnostics {
        let anchor = match &diagnostic {
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::InvalidCondensationGraph => DiagnosticAnchor::Project,
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::MissingBody { unit } => DiagnosticAnchor::Unit(*unit),
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::MissingSummary { call, .. } => DiagnosticAnchor::Expr(*call),
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::IterationLimitReached { component } => component.first().copied().map(DiagnosticAnchor::Unit).unwrap_or(DiagnosticAnchor::Project),
        };
        effects.diagnostics.push(materialize_effect_diagnostic(
            hir,
            etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            format!("trace spec interprocedural analysis failed: {diagnostic:?}"),
        )?);
    }
    for convergence in convergence {
        if convergence.status == etas_utils::ConvergenceStatus::IterationLimitReached {
            for unit in &convergence.units {
                if let Some(owner) = unit.owner() {
                    reject_item_for_trace_spec(effects, owner);
                }
            }
            let anchor = convergence
                .units
                .first()
                .copied()
                .map(DiagnosticAnchor::Unit)
                .unwrap_or(DiagnosticAnchor::Project);
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                anchor,
                "trace spec summary solving reached the iteration limit",
            )?);
        }
    }
    for item in rejected_items {
        reject_item_for_trace_spec(effects, item);
    }

    Ok(TraceSpecAnalysisOutput { summaries })
}
