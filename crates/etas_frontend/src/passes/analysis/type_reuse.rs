use etas_utils::{
    ArtifactRef, ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts, UnitKey,
};

use crate::incremental::{BodyArtifactIdentity, dependency_fingerprints_match};
use crate::passes::artifacts::{AFFECTED_MODULE_SET, TYPE_OUTPUT, UNIT_TREE};
use crate::{BODY_UNIT_KIND, ProjectContext};

pub struct ReuseTypeBodyFactsPass;

impl Pass<ProjectContext> for ReuseTypeBodyFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("ReuseTypeBodyFactsPass", PassKind::Analysis)
            .requires(ArtifactSet::from([UNIT_TREE, AFFECTED_MODULE_SET]))
            .produces(ArtifactSet::from_iter([ArtifactRef::unit_kind(
                TYPE_OUTPUT,
                BODY_UNIT_KIND,
            )]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if !context.incremental || context.body_artifact_reuse.type_outputs.is_empty() {
            return PassResult::unchanged();
        }

        let Some(units) = context.units.as_ref() else {
            return PassResult::failed("unit tree should exist before reusing body type facts");
        };
        let Some(sources) = context.sources.as_ref() else {
            return PassResult::failed("source set should exist before reusing body type facts");
        };

        let current_manifest = context.current_artifact_manifest();
        let mut produced = ArtifactSet::new();
        let mut reused = context.body_artifact_reuse.type_outputs.clone();
        reused.sort_by_key(|artifact| artifact.unit.0);

        for artifact in reused {
            let unit_key = UnitKey::new(BODY_UNIT_KIND, artifact.unit.0 as u64);
            if context.type_body_outputs.contains_key(&artifact.unit) {
                continue;
            }
            let Some(current_identity) =
                BodyArtifactIdentity::for_unit(artifact.unit, units, sources)
            else {
                continue;
            };
            if current_identity != artifact.identity {
                continue;
            }
            if !dependency_fingerprints_match(&artifact.dependencies, &current_manifest) {
                continue;
            }
            context
                .type_body_outputs
                .insert(artifact.unit, artifact.output);
            context.reused_cache_artifacts.push(artifact.cache_key);
            produced.insert_ref(ArtifactRef::unit(TYPE_OUTPUT, unit_key));
        }

        if produced.is_empty() {
            PassResult::unchanged()
        } else {
            PassResult::changed(PreservedArtifacts::All, produced)
        }
    }
}

#[cfg(test)]
mod tests {
    use etas_utils::{PassControl, PassManager, UnitKey};

    use super::*;
    use crate::artifact::{FrontendArtifactKey, FrontendArtifactKind};
    use crate::incremental::{BodyArtifactReuseInput, CachedTypeBodyArtifact};
    use crate::pipeline::project_pipeline;
    use crate::{ProjectInput, SourceInput, UnitKind};

    #[test]
    fn incremental_pipeline_reuses_cached_type_body_outputs_before_type_checking() {
        let input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return helper();
}
"#,
        ));
        let mut full_context = ProjectContext::new(input.clone());
        let mut full_pipeline = project_pipeline();
        let mut full_manager = PassManager::new();
        let full_run = full_manager.run_pipeline(&mut full_pipeline, &mut full_context);

        assert!(
            matches!(full_run.control, PassControl::Continue),
            "{:?}",
            full_run.control
        );
        let initial_type_checks = full_run
            .records
            .iter()
            .filter(|record| record.pass == "TypeCheckBodyPass")
            .count();
        assert_eq!(initial_type_checks, 2);

        let units = full_context.units.as_ref().expect("unit tree should exist");
        let sources = full_context
            .sources
            .as_ref()
            .expect("source set should exist");
        let mut type_outputs = full_context
            .type_body_outputs
            .iter()
            .map(|(unit, output)| {
                let unit_key = UnitKey::new(BODY_UNIT_KIND, unit.0 as u64);
                CachedTypeBodyArtifact {
                    unit: *unit,
                    cache_key: FrontendArtifactKey::unit(FrontendArtifactKind::TypeFacts, unit_key)
                        .to_cache_key(),
                    identity: BodyArtifactIdentity::for_unit(*unit, units, sources)
                        .expect("body identity should resolve"),
                    dependencies: Vec::new(),
                    output: output.clone(),
                }
            })
            .collect::<Vec<_>>();
        type_outputs.sort_by_key(|artifact| artifact.unit.0);

        let mut incremental_context = ProjectContext::new_incremental_with_reuse(
            input,
            Vec::new(),
            false,
            BodyArtifactReuseInput {
                type_outputs,
                effect_outputs: Vec::new(),
            },
            std::collections::HashMap::new(),
        );
        let mut incremental_pipeline = project_pipeline();
        let mut incremental_manager = PassManager::new();
        let incremental_run =
            incremental_manager.run_pipeline(&mut incremental_pipeline, &mut incremental_context);

        assert!(
            matches!(incremental_run.control, PassControl::Continue),
            "{:?}",
            incremental_run.control
        );
        assert!(incremental_context.checked.is_some());
        assert_eq!(
            incremental_context
                .type_body_outputs
                .keys()
                .filter(|unit| {
                    incremental_context
                        .units
                        .as_ref()
                        .expect("unit tree should exist")
                        .nodes
                        .get(**unit)
                        .is_some_and(|node| node.kind == UnitKind::Body)
                })
                .count(),
            2
        );
        assert_eq!(incremental_context.reused_cache_artifacts.len(), 2);
        assert!(
            incremental_run
                .records
                .iter()
                .all(|record| record.pass != "TypeCheckBodyPass")
        );
    }
}
