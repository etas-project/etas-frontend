use crate::pipeline::EffectAnalysisMode;

use super::*;

pub(super) struct ReplaySummariesPass;

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
