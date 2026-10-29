use super::*;

pub(crate) struct SolveSummariesPass;

impl<'a> Pass<EffectPipelineContext<'a>> for SolveSummariesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.solve_summaries", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                EFFECT_UNITS,
                MEMORY_PROVENANCE,
                FROZEN_CALL_SUMMARIES,
            ]))
            .produces(ArtifactSet::one(EFFECT_SUMMARIES))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::SolveSummaries, "effect registry"));
        };
        let Some(units) = context.units.as_deref() else {
            return context.fail(missing(EffectStage::SolveSummaries, "effect units"));
        };
        let Some(memory_provenance) = context.memory_provenance.as_ref() else {
            return context.fail(missing(EffectStage::SolveSummaries, "memory provenance"));
        };
        let Some(frozen_calls) = context.frozen_calls.as_ref() else {
            return context.fail(missing(
                EffectStage::SolveSummaries,
                "frozen call summaries",
            ));
        };
        let parameter_effects = std::sync::Arc::new(frozen_calls.parameter_effects.clone());
        let analysis = run_effect_analysis(
            EffectAnalysisInput {
                external_implementations: context.input.external_implementations,
                method_evidence: std::sync::Arc::new(frozen_calls.method_evidence.clone()),
                frozen_graphs: std::sync::Arc::new(frozen_calls.graphs.clone()),
                callable_environments: std::sync::Arc::new(
                    frozen_calls.callable_environments.clone(),
                ),
                global_values: Default::default(),
                call_results: Default::default(),
                captured_values: Default::default(),
                frozen_calls: std::sync::Arc::new(frozen_calls.callables.clone()),
                parameter_effects: parameter_effects.clone(),
                frozen_parameter_effects: parameter_effects,
                hir: context.input.hir,
                types: context.input.types,
                std_registry: context.input.std_registry,
                memory_provenance: memory_provenance.clone(),
                registry,
                tool_bindings: context.input.tool_bindings,
                external_summaries: context.input.external_summaries,
            },
            units,
            manager.execution_control(),
        );
        match analysis {
            Ok(analysis) => context.analysis = Some(analysis),
            Err(error) => return context.fail(error),
        }
        produced(EFFECT_SUMMARIES)
    }
}
