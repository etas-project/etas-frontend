use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use crate::{
    ProjectContext,
    code::reflection::{CheckedReflectionTargets, ReflectionTargetError},
    passes::artifacts::{
        ANALYSIS_REACHABILITY_FACTS, CHECKED_REFLECTION_TARGETS, HIR_OUTPUT, RESOLVED_PATHS,
        TYPE_OUTPUT,
    },
};

pub(crate) struct ResolveReflectionTargetsPass;

impl Pass<ProjectContext> for ResolveReflectionTargetsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ResolveReflectionTargetsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                RESOLVED_PATHS,
                TYPE_OUTPUT,
                ANALYSIS_REACHABILITY_FACTS,
            ]))
            .produces(ArtifactSet::one(CHECKED_REFLECTION_TARGETS))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(hir), Some(paths), Some(types), Some(reachability)) = (
            &context.hir,
            &context.resolved_paths,
            &context.types,
            &context.reachability,
        ) else {
            return PassResult::failed(
                "reflection targets require HIR, resolved paths, type facts and analysis scope",
            );
        };
        let targets = match CheckedReflectionTargets::resolve(
            &hir.hir,
            paths,
            types,
            &reachability.analysis_items,
            context
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
            manager.execution_control(),
        ) {
            Ok(targets) => targets,
            Err(ReflectionTargetError::Invalid(reason)) => return PassResult::failed(reason),
            Err(ReflectionTargetError::Interrupted(interruption)) => {
                return PassResult::interrupted(interruption);
            }
        };
        context.reflection_targets = Some(std::sync::Arc::new(targets));
        PassResult::changed(PreservedArtifacts::All, self.descriptor().produces)
    }
}

#[cfg(test)]
mod tests;
