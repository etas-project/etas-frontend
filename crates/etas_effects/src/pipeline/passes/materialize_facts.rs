use super::*;

pub(crate) struct MaterializeFactsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for MaterializeFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.materialize_facts", PassKind::Analysis)
            .requires(ArtifactSet::from([EFFECT_REGISTRY, EFFECT_SUMMARIES]))
            .produces(ArtifactSet::one(EFFECT_FACTS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::MaterializeFacts, "effect registry"));
        };
        let Some(analysis) = context.analysis.take() else {
            return context.fail(missing(
                EffectStage::MaterializeFacts,
                "solved effect summaries",
            ));
        };
        if matches!(
            context.analysis_mode,
            crate::pipeline::EffectAnalysisMode::Capture
        ) {
            context.captured_analysis = Some(std::sync::Arc::new(analysis.clone()));
        }
        let effects = crate::facts::EffectFactProjector::materialize(
            context.input.hir,
            context.input.types,
            registry,
            analysis,
            context.input.reachable_items,
        );
        match effects {
            Ok(mut effects) => {
                let imported =
                    crate::suspension::imported_contracts(
                        context
                            .input
                            .external_summaries
                            .iter()
                            .filter_map(|summary| summary.metadata.callable_contract.as_ref())
                            .chain(
                                context.input.external_implementations.iter().filter_map(
                                    |method| method.metadata.callable_contract.as_ref(),
                                ),
                            ),
                    );
                effects.facts.suspension = match crate::suspension::analyze(
                    context.input.hir,
                    context.input.types,
                    &effects.facts,
                    context.input.std_registry,
                    &imported,
                    manager.execution_control(),
                ) {
                    Ok(suspension) => suspension,
                    Err(error) => return context.fail(EffectPipelineError::Interrupted(error)),
                };
                crate::updating::validate(
                    context.input.hir,
                    context.input.types,
                    &mut effects,
                    &imported,
                    context.input.reachable_items,
                );
                crate::parameter_defaults::validate(
                    context.input.hir,
                    &mut effects,
                    context.input.reachable_items,
                );
                if let crate::pipeline::EffectAnalysisMode::CapturePrepared(capture) =
                    context.analysis_mode
                {
                    assert!(capture.set(effects.clone()).is_ok());
                }
                context.effects = Some(effects);
            }
            Err(error) => return context.fail(error),
        }
        produced(EFFECT_FACTS)
    }
}
