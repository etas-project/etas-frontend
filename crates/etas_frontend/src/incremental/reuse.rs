use etas_cache::{ArtifactKey, ArtifactMeta};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheReuseReport {
    pub reused_artifacts: Vec<ArtifactKey>,
    pub stored_artifacts: Vec<ArtifactKey>,
}

impl CacheReuseReport {
    pub(crate) fn record_reused(&mut self, key: ArtifactKey) {
        if !self.reused_artifacts.contains(&key) {
            self.reused_artifacts.push(key);
        }
    }

    pub(crate) fn record_reused_many(&mut self, keys: impl IntoIterator<Item = ArtifactKey>) {
        for key in keys {
            self.record_reused(key);
        }
    }

    pub(crate) fn record_stored(&mut self, key: ArtifactKey) {
        if !self.stored_artifacts.contains(&key) {
            self.stored_artifacts.push(key);
        }
    }
}

pub(crate) fn artifact_meta_matches(stored: &ArtifactMeta, expected: &ArtifactMeta) -> bool {
    stored.fingerprint == expected.fingerprint
        && stored.compiler_version == expected.compiler_version
        && stored.cache_schema_version == expected.cache_schema_version
        && stored.std_version == expected.std_version
        && stored.options_hash == expected.options_hash
        && stored.dependencies == expected.dependencies
}
