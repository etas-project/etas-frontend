use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    passes::artifacts::{
        ANALYSIS_REACHABILITY_FACTS, DEFINITION_SOURCES, HIR_OUTPUT, REACHABILITY_FACTS,
        RESOLVED_PATHS,
    },
};

pub struct ComputeDefinitionAnalysisReachabilityPass;

impl Pass<ProjectContext> for ComputeDefinitionAnalysisReachabilityPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new(
            "ComputeDefinitionAnalysisReachabilityPass",
            PassKind::Analysis,
        )
        .requires(ArtifactSet::from([
            DEFINITION_SOURCES,
            REACHABILITY_FACTS,
            HIR_OUTPUT,
            RESOLVED_PATHS,
        ]))
        .produces(ArtifactSet::one(ANALYSIS_REACHABILITY_FACTS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        _: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let mut roots = if context.check_scope == crate::CheckScope::FullProject {
            let Some(hir) = &context.hir else {
                return PassResult::failed("project analysis requires finalized HIR");
            };
            hir.hir
                .items
                .iter()
                .map(|(item, _)| item)
                .collect::<Vec<_>>()
        } else {
            let Some(sources) = &context.definition_sources else {
                return if context
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == etas_core::Severity::Error)
                {
                    PassResult::stop()
                } else {
                    PassResult::failed("definition analysis requires a validated source archive")
                };
            };
            sources.retained_items().collect::<Vec<_>>()
        };
        roots.sort();
        let (items, bodies) =
            match super::runtime_requirements::source_analysis_closure(context, roots) {
                Ok(closure) => closure,
                Err(reason) => return PassResult::failed(reason),
            };
        let Some(reachability) = &mut context.reachability else {
            return PassResult::failed("definition analysis requires entry reachability facts");
        };
        // Retained code needs checked behavior, not execution authority or source overlays.
        reachability.analysis_items = reachability
            .reachable_items
            .union(&items)
            .copied()
            .collect();
        reachability.analysis_bodies = reachability
            .reachable_bodies
            .union(&bodies)
            .copied()
            .collect();
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(ANALYSIS_REACHABILITY_FACTS),
        )
    }
}
