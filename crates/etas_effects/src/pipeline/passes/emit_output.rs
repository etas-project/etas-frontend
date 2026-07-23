use super::*;

pub(crate) struct EmitOutputPass;

impl<'a> Pass<EffectPipelineContext<'a>> for EmitOutputPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.emit_output", PassKind::Emit)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                EFFECT_UNITS,
                VALIDATED_TRACE_SPEC_FACTS,
            ]))
            .produces(ArtifactSet::one(EFFECT_OUTPUT))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.take() else {
            return context.fail(missing(EffectStage::Emit, "effect registry"));
        };
        let Some(units) = context.units.take() else {
            return context.fail(missing(EffectStage::Emit, "effect units"));
        };
        let Some(mut effects) = context.effects.take() else {
            return context.fail(missing(EffectStage::Emit, "validated effect output"));
        };
        let artifacts = EffectPipelineArtifacts {
            registry,
            dependency_metadata: context
                .input
                .dependency_metadata
                .cloned()
                .unwrap_or_default(),
            tool_bindings: context.input.tool_bindings.to_vec(),
            external_summaries: context
                .input
                .external_summaries
                .iter()
                .map(|summary| summary.metadata.clone())
                .collect(),
            external_trace_specs: context
                .input
                .external_trace_specs
                .iter()
                .map(|summary| summary.metadata.clone())
                .collect(),
            units,
            pass_timings: Vec::new(),
        };
        effects
            .diagnostics
            .append(&mut context.registry_diagnostics);
        context.effects = Some(effects);
        context.artifacts = Some(artifacts);
        produced(EFFECT_OUTPUT)
    }
}
