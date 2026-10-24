use etas_utils::UnitKey;

use crate::incremental::{BodyArtifactIdentity, dependency_fingerprints_match};
use crate::{BODY_UNIT_KIND, ProjectContext};

pub(super) fn record_reused_effect_body_artifacts(context: &mut ProjectContext) {
    if !context.incremental || context.body_artifact_reuse.effect_outputs.is_empty() {
        return;
    }
    let Some(units) = context.units.as_ref() else {
        return;
    };
    let Some(sources) = context.sources.as_ref() else {
        return;
    };
    let current_manifest = context.current_artifact_manifest();
    let mut reused = context.body_artifact_reuse.effect_outputs.clone();
    reused.sort_by_key(|artifact| artifact.unit.0);

    for artifact in reused {
        if !context.type_body_outputs.contains_key(&artifact.unit) {
            continue;
        }
        let unit_key = UnitKey::new(BODY_UNIT_KIND, artifact.unit.0 as u64);
        if context.effect_body_outputs.contains_key(&artifact.unit) {
            continue;
        }
        let Some(current_identity) = BodyArtifactIdentity::for_unit(artifact.unit, units, sources)
        else {
            continue;
        };
        if current_identity != artifact.identity {
            continue;
        }
        if !dependency_fingerprints_match(&artifact.dependencies, &current_manifest) {
            continue;
        }
        let expected_key = crate::artifact::FrontendArtifactKey::unit(
            crate::artifact::FrontendArtifactKind::EffectFacts,
            unit_key,
        )
        .to_cache_key();
        if artifact.cache_key == expected_key
            && !context.reused_cache_artifacts.contains(&artifact.cache_key)
        {
            context.reused_cache_artifacts.push(artifact.cache_key);
        }
    }
}
