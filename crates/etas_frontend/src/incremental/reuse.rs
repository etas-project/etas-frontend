use etas_cache::{ArtifactKey, ArtifactMeta};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheReuseReport {
    pub reused_artifacts: Vec<ArtifactKey>,
    /// Accepted writes; later dependency eviction may remove these entries.
    pub stored_artifacts: Vec<ArtifactKey>,
    pub skipped_memory_budget_artifacts: Vec<ArtifactKey>,
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

    pub(crate) fn record_put(&mut self, key: ArtifactKey, status: etas_cache::ArtifactPutStatus) {
        let keys = match status {
            etas_cache::ArtifactPutStatus::Stored => &mut self.stored_artifacts,
            etas_cache::ArtifactPutStatus::SkippedMemoryBudget => {
                &mut self.skipped_memory_budget_artifacts
            }
        };
        if !keys.contains(&key) {
            keys.push(key);
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
