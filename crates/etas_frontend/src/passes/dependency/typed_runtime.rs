use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    passes::artifacts::{
        ANALYSIS_REACHABILITY_FACTS, HIR_OUTPUT, MODULE_INDEX, PROJECT_ANALYSIS_ROOTS,
        PROJECT_ENTRY, REACHABILITY_FACTS, RESOLVED_PATHS, RUNTIME_SOURCE_REQUIREMENTS,
        TYPE_OUTPUT, VALIDATED_EXTERNAL_ENVIRONMENT,
    },
};

pub struct CompleteTypedRuntimeReachabilityPass;

impl Pass<ProjectContext> for CompleteTypedRuntimeReachabilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("CompleteTypedRuntimeReachabilityPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                RESOLVED_PATHS,
                PROJECT_ENTRY,
                PROJECT_ANALYSIS_ROOTS,
                ANALYSIS_REACHABILITY_FACTS,
                REACHABILITY_FACTS,
                TYPE_OUTPUT,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::from([
                REACHABILITY_FACTS,
                RUNTIME_SOURCE_REQUIREMENTS,
            ]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(types), Some(previous), Some(environment)) = (
            &context.types,
            &context.reachability,
            &context.validated_external_environment,
        ) else {
            return PassResult::failed(
                "runtime reachability requires checked types, analysis roots and validated metadata",
            );
        };
        let mut facts = match super::runtime_requirements::compute_reachability(
            context,
            environment.environment(),
            Some(&types.facts),
        ) {
            Ok(facts) => facts,
            Err(error) => return PassResult::failed(error),
        };
        if !facts.reachable_items.is_subset(&previous.analysis_items)
            || !facts.reachable_bodies.is_subset(&previous.analysis_bodies)
        {
            return PassResult::failed(
                "runtime reachability includes a body outside checked analysis",
            );
        }
        facts.analysis_items = previous.analysis_items.clone();
        facts.analysis_bodies = previous.analysis_bodies.clone();
        context.runtime_source_requirements = Some(facts.runtime_source_requirements.clone());
        context.reachability = Some(facts);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::from([REACHABILITY_FACTS, RUNTIME_SOURCE_REQUIREMENTS]),
        )
    }
}
