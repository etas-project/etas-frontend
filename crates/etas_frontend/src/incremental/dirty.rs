use etas_cache::ArtifactKey;
use etas_core::SourceId;

use crate::artifact::{FrontendArtifactKey, FrontendArtifactKind};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirtyArtifactRoots {
    keys: Vec<ArtifactKey>,
}

impl DirtyArtifactRoots {
    pub(crate) fn from_sources(changed_sources: &[SourceId], broad_project_change: bool) -> Self {
        let mut keys = changed_sources
            .iter()
            .flat_map(|source| {
                [
                    FrontendArtifactKey::source(FrontendArtifactKind::ParsedSource, *source)
                        .to_cache_key(),
                    FrontendArtifactKey::source(FrontendArtifactKind::Diagnostics, *source)
                        .to_cache_key(),
                ]
            })
            .collect::<Vec<_>>();

        keys.push(FrontendArtifactKey::project(FrontendArtifactKind::SourceSet).to_cache_key());

        if broad_project_change {
            keys.push(
                FrontendArtifactKey::project(FrontendArtifactKind::ModuleIndex).to_cache_key(),
            );
            keys.push(
                FrontendArtifactKey::project(FrontendArtifactKind::ImportGraph).to_cache_key(),
            );
            keys.push(
                FrontendArtifactKey::project(FrontendArtifactKind::ResolvedImports).to_cache_key(),
            );
            keys.push(
                FrontendArtifactKey::project(FrontendArtifactKind::ResolvedPaths).to_cache_key(),
            );
        }

        keys.sort();
        keys.dedup();
        Self { keys }
    }

    pub(crate) fn into_keys(self) -> Vec<ArtifactKey> {
        self.keys
    }
}
