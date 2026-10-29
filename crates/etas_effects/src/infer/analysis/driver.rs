use etas_hir_analysis::interprocedural::InterproceduralAnalysis;

use crate::{EffectPipelineError, EffectUnit, facts::EffectAnalysisOutput};

use super::{input::EffectAnalysisInput, output::output_from_result};
use crate::infer::semantics::engine::EffectSemantics;

pub fn run_effect_analysis<'a>(
    input: EffectAnalysisInput<'a>,
    units: &[EffectUnit],
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
) -> Result<EffectAnalysisOutput, EffectPipelineError> {
    let mut output = None;
    let analysis_units = std::sync::Arc::new(
        units
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
    );
    let argument_writes = std::sync::Arc::new(
        crate::infer::semantics::argument_values::ArgumentWrites::build(input.hir, control)
            .map_err(EffectPipelineError::Interrupted)?,
    );
    let solved = etas_utils::FixpointEngine::default().try_solve(input, |input| {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        let mut semantics = EffectSemantics::from_input(input.clone(), argument_writes.clone());
        semantics.analysis_units = analysis_units.clone();
        semantics.query_control = control;
        let analysis = InterproceduralAnalysis::new(units.iter().copied(), semantics);
        let mut result = match control {
            Some(control) => analysis
                .solve_controlled(control)
                .map_err(EffectPipelineError::Interrupted)?,
            None => analysis.solve(),
        };
        if let Some(interruption) = result
            .semantics
            .callable_summaries
            .borrow_mut()
            .interruption
            .take()
        {
            return Err(EffectPipelineError::Interrupted(interruption));
        }
        let mut parameters = (*input.parameter_effects).clone();
        let callable_environments = result.semantics.callable_environments.borrow().clone();
        for (symbol, observed) in &result.semantics.inputs.inferred_parameter_effects {
            let mut summary = observed.clone();
            if let Some(current) = parameters.get(symbol) {
                summary.join_branch(current);
            }
            summary.stabilize_for_fixpoint(parameters.get(symbol));
            parameters.insert(*symbol, summary);
        }
        let mut captured_values = (*input.captured_values).clone();
        for (unit, captures) in &result.semantics.discovered_value_captures {
            captured_values
                .entry(*unit)
                .or_default()
                .join_assign(captures);
        }
        let mut global_values = (*input.global_values).clone();
        global_values.join_assign(&result.semantics.discovered_global_values);
        let call_results = result.semantics.discovered_call_results.borrow().clone();
        if parameters == *input.parameter_effects
            && captured_values == *input.captured_values
            && global_values == *input.global_values
            && call_results == *input.call_results
            && callable_environments == input.callable_environments
        {
            result.semantics.inputs.callable_environments = (*callable_environments).clone();
            result.semantics.inputs.inferred_parameter_effects = parameters;
            output = Some(output_from_result(result)?);
            return Ok(false);
        }
        // Re-run transfers with solved parameter behavior and captured value
        // sources, so invocation effects pass through normal handlers and control flow.
        input.parameter_effects = std::sync::Arc::new(parameters);
        input.captured_values = std::sync::Arc::new(captured_values);
        input.global_values = std::sync::Arc::new(global_values);
        input.call_results = std::sync::Arc::new(call_results);
        input.callable_environments = callable_environments;
        Ok(true)
    })?;
    if solved.status != etas_utils::ConvergenceStatus::Converged {
        return Err(EffectPipelineError::AnalysisDidNotConverge {
            stage: crate::EffectStage::SolveSummaries,
            iterations: solved.stats.iterations,
        });
    }
    output.ok_or(EffectPipelineError::MissingStageArtifact {
        stage: crate::EffectStage::SolveSummaries,
        artifact: "converged effect analysis",
    })
}
