use etas_cache::{ArtifactFingerprint, ArtifactKey};

use super::key::FrontendArtifactKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontendArtifactDependency {
    pub key: FrontendArtifactKey,
    pub fingerprint: Option<ArtifactFingerprint>,
}

pub fn cache_dependencies(dependencies: &[FrontendArtifactDependency]) -> Vec<ArtifactKey> {
    let mut keys = dependencies
        .iter()
        .map(|dependency| dependency.key.to_cache_key())
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}
