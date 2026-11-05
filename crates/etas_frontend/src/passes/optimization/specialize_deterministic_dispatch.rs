use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult, PassScope,
    PreservedArtifacts,
};

use crate::passes::artifacts::{EFFECT_OUTPUT, HIR_OPTIMIZATION_LEGALITY, HIR_OUTPUT, TYPE_OUTPUT};
use crate::{BODY_UNIT_KIND, HirOptimizationGroup, HirOptimizationPassReport, ProjectContext};

pub struct SpecializeDeterministicDispatchPass;

impl Pass<ProjectContext> for SpecializeDeterministicDispatchPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("SpecializeDeterministicDispatchPass", PassKind::Transform)
            .scope(PassScope::Unit(BODY_UNIT_KIND))
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                HIR_OPTIMIZATION_LEGALITY,
            ]))
            .produces(ArtifactSet::from([HIR_OUTPUT, TYPE_OUTPUT, EFFECT_OUTPUT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let body = crate::optimization::current_optimization_body(context, pass_context);
        let outcome = super::record_projection::project_marked_records(
            context,
            &body.expressions,
            "DeterministicAgentDispatch",
        );
        context
            .optimization_report
            .record(HirOptimizationPassReport::new(
                "SpecializeDeterministicDispatch",
                HirOptimizationGroup::O2,
                outcome.rewritten,
                outcome.rewritten,
                outcome.affected_units,
                outcome.rewritten != 0,
            ));
        if outcome.rewritten == 0 {
            PassResult::unchanged()
        } else {
            PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
        }
    }
}
