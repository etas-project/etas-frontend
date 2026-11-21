use crate::pipeline::EffectAnalysisMode;

use super::*;

pub(super) struct ReplaySummariesPass;
pub(super) struct ReplayPreparedFactsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for ReplayPreparedFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.replay_prepared_facts", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                EFFECT_UNITS,
                FROZEN_CALL_SUMMARIES,
            ]))
            .produces(ArtifactSet::one(EFFECT_FACTS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _: &PassContext<EffectPipelineContext<'a>>,
        _: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let EffectAnalysisMode::ReplayPrepared(effects) = context.analysis_mode else {
            return context.fail(missing(
                EffectStage::MaterializeFacts,
                "installed Effect facts",
            ));
        };
        context.effects = Some(effects.clone());
        produced(EFFECT_FACTS)
    }
}

impl<'a> Pass<EffectPipelineContext<'a>> for ReplaySummariesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.replay_summaries", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                EFFECT_UNITS,
                FROZEN_CALL_SUMMARIES,
            ]))
            .produces(ArtifactSet::one(EFFECT_SUMMARIES))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let EffectAnalysisMode::Replay(analysis) = context.analysis_mode else {
            return context.fail(missing(
                EffectStage::SolveSummaries,
                "archived effect analysis",
            ));
        };
        context.analysis = Some(analysis.clone());
        produced(EFFECT_SUMMARIES)
    }
}
