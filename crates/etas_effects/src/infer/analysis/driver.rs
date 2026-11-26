use etas_hir_analysis::interprocedural::SummarySolver;
use std::collections::HashSet;

use crate::{EffectPipelineError, EffectUnit, facts::EffectAnalysisOutput};

use super::{input::EffectAnalysisInput, output::output_from_result};
use crate::infer::semantics::{engine::EffectSemantics, type_queries::EffectTypeQueries};

pub fn run_effect_analysis<'a>(
    input: EffectAnalysisInput<'a>,
    units: &[EffectUnit],
    control: Option<&'a dyn etas_utils::PipelineExecutionControl>,
) -> Result<EffectAnalysisOutput, EffectPipelineError> {
    let _fingerprints = crate::infer::domain::TraceFingerprintScope::enter();
    let mut previous = None;
    let mut dirty = HashSet::new();
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
    let latent_type_shapes = std::sync::Arc::new(std::sync::OnceLock::new());
    let type_queries = std::rc::Rc::new(EffectTypeQueries::default());
    let solved = etas_utils::FixpointEngine::default().try_solve(input, |input| {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        let mut semantics = EffectSemantics::from_input(
            input.clone(),
            argument_writes.clone(),
            type_queries.clone(),
        );
        semantics.latent_type_shapes = latent_type_shapes.clone();
        semantics.analysis_units = analysis_units.clone();
        semantics.query_control = control;
        let first = previous.is_none();
        let mut old_summaries = Default::default();
        let cached = previous.take().map(|(prior, solved)| {
            old_summaries = semantics.reuse_clean(prior, &dirty);
            solved
        });
        let result = SummarySolver::default()
            .solve_incremental(
                units.iter().copied(),
                &mut semantics,
                cached,
                |unit| first || unit.owner().is_some_and(|item| dirty.contains(&item)),
                &|| control.map_or(Ok(()), |control| control.checkpoint()),
            )
            .map_err(EffectPipelineError::Interrupted)?;
        semantics.select_reader(None);
        if let Some(interruption) = semantics
            .callable_summaries
            .borrow_mut()
            .interruption
            .take()
        {
            return Err(EffectPipelineError::Interrupted(interruption));
        }
        let mut parameters = (*input.parameter_effects).clone();
        let callable_environments = semantics.callable_environments.borrow().clone();
        let external_parameter_projections =
            semantics.external_parameter_projections.borrow().clone();
        let parameter_projections = semantics.parameter_projections.borrow().clone();
        let method_evidence = semantics.method_evidence.borrow().clone();
        let mut spec_method_targets = (*input.spec_method_targets).clone();
        let mut spec_unit_targets = (*input.spec_unit_targets).clone();
        for (owner, targets) in semantics.discovered_spec_unit_targets.borrow().iter() {
            spec_unit_targets.entry(*owner).or_default().extend(targets);
        }
        for (call, targets) in semantics.discovered_spec_targets.borrow().iter() {
            spec_method_targets
                .entry(*call)
                .or_default()
                .extend(targets);
        }
        for (symbol, observed) in &semantics.inputs.inferred_parameter_effects {
            let mut summary = observed.clone();
            if let Some(current) = parameters.get(symbol) {
                summary.join_branch(current);
            }
            summary.stabilize_for_fixpoint(parameters.get(symbol));
            parameters.insert(*symbol, summary);
        }
        let mut captured_values = (*input.captured_values).clone();
        for (unit, captures) in &semantics.discovered_value_captures {
            captured_values
                .entry(*unit)
                .or_default()
                .join_assign(captures);
        }
        let mut global_values = (*input.global_values).clone();
        global_values.join_assign(&semantics.discovered_global_values);
        let call_results = semantics.discovered_call_results.borrow().origins.clone();
        let changed_summaries: HashSet<_> = old_summaries
            .into_iter()
            .filter_map(|(unit, old)| {
                (semantics.inputs.unit_effects.get(&unit) != Some(&old))
                    .then(|| unit.owner())
                    .flatten()
            })
            .collect();
        let stable_inputs = parameters == *input.parameter_effects
            && captured_values == *input.captured_values
            && global_values == *input.global_values
            && call_results == *input.call_results
            && callable_environments == input.callable_environments
            && parameter_projections == input.parameter_projections
            && external_parameter_projections == input.external_parameter_projections
            && method_evidence == input.method_evidence
            && spec_method_targets == *input.spec_method_targets
            && spec_unit_targets == *input.spec_unit_targets;
        // Re-run transfers with solved parameter behavior and captured value
        // sources, so invocation effects pass through normal handlers and control flow.
        let old = input.clone();
        input.parameter_effects = std::sync::Arc::new(parameters);
        input.captured_values = std::sync::Arc::new(captured_values);
        input.global_values = std::sync::Arc::new(global_values);
        input.call_results = std::sync::Arc::new(call_results);
        input.callable_environments = callable_environments;
        input.parameter_projections = parameter_projections;
        input.external_parameter_projections = external_parameter_projections;
        input.method_evidence = method_evidence;
        // Witness applications discover body dependencies after substitution.
        input.spec_method_targets = std::sync::Arc::new(spec_method_targets);
        input.spec_unit_targets = std::sync::Arc::new(spec_unit_targets);
        let visited = std::mem::take(&mut dirty);
        let (next_dirty, pending_summaries) = semantics.dependencies.invalidated(
            &old,
            input,
            &result.call_graph,
            &visited,
            &changed_summaries,
            &result.convergence,
        );
        dirty = next_dirty;
        if stable_inputs && !pending_summaries {
            semantics.inputs.inferred_parameter_effects = (*input.parameter_effects).clone();
            output = Some(output_from_result(semantics, result)?);
            return Ok(false);
        }
        if !result.diagnostics.is_empty() {
            dirty.extend(
                result
                    .call_graph
                    .units()
                    .into_iter()
                    .filter_map(EffectUnit::owner),
            );
        }
        previous = Some((semantics, result));
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
