use etas_cache::{ArtifactFingerprint, ArtifactKey, ProjectRevision};
use etas_core::SourceId;

use super::{
    PersistedFrontendArtifactDependency, PersistedFrontendArtifactKey,
    dependency::FrontendArtifactDependency, fingerprint_text, key::FrontendArtifactKey,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontendArtifactRecord {
    pub key: FrontendArtifactKey,
    pub revision: ProjectRevision,
    pub fingerprint: ArtifactFingerprint,
    pub dependencies: Vec<FrontendArtifactDependency>,
    pub diagnostics: Vec<SourceId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontendArtifactManifest {
    pub records: Vec<FrontendArtifactRecord>,
}

impl FrontendArtifactManifest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, record: FrontendArtifactRecord) {
        self.records.retain(|existing| existing.key != record.key);
        self.records.push(record);
    }

    pub fn get(&self, key: &FrontendArtifactKey) -> Option<&FrontendArtifactRecord> {
        self.records.iter().find(|record| &record.key == key)
    }

    pub fn cache_keys(&self) -> impl Iterator<Item = ArtifactKey> + '_ {
        self.records.iter().map(|record| record.key.to_cache_key())
    }

    pub fn fingerprint(&self) -> ArtifactFingerprint {
        let mut records = self.records.iter().collect::<Vec<_>>();
        records.sort_by_key(|record| record.key.to_cache_key().to_string());
        let mut parts = vec!["frontend_artifact_manifest:v1".to_owned()];
        for record in records {
            parts.extend(record.fingerprint_parts());
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        fingerprint_text(&refs)
    }

    pub(crate) fn to_persisted(&self) -> PersistedFrontendArtifactManifest {
        PersistedFrontendArtifactManifest {
            records: self
                .records
                .iter()
                .map(PersistedFrontendArtifactRecord::from_frontend)
                .collect(),
        }
    }

    pub(crate) fn from_persisted(persisted: PersistedFrontendArtifactManifest) -> Option<Self> {
        Some(Self {
            records: persisted
                .records
                .into_iter()
                .map(PersistedFrontendArtifactRecord::into_frontend)
                .collect::<Option<Vec<_>>>()?,
        })
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl FrontendArtifactRecord {
    fn fingerprint_parts(&self) -> Vec<String> {
        let mut parts = vec![
            format!("key:{}", self.key.to_cache_key()),
            format!("revision:{}", self.revision.0),
            format!("fingerprint:{:?}", self.fingerprint.bytes()),
        ];
        for dependency in &self.dependencies {
            parts.push(format!("dependency:{}", dependency.key.to_cache_key()));
            parts.push(format!(
                "dependency_fingerprint:{:?}",
                dependency.fingerprint.map(ArtifactFingerprint::bytes)
            ));
        }
        for source in &self.diagnostics {
            parts.push(format!("diagnostic_source:{}", source.0));
        }
        parts
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedFrontendArtifactManifest {
    pub records: Vec<PersistedFrontendArtifactRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedFrontendArtifactRecord {
    pub key: PersistedFrontendArtifactKey,
    pub revision: u64,
    pub fingerprint: [u8; 32],
    pub dependencies: Vec<PersistedFrontendArtifactDependency>,
    pub diagnostics: Vec<u32>,
}

impl PersistedFrontendArtifactRecord {
    fn from_frontend(record: &FrontendArtifactRecord) -> Self {
        Self {
            key: PersistedFrontendArtifactKey::from_frontend(&record.key),
            revision: record.revision.0,
            fingerprint: record.fingerprint.bytes(),
            dependencies: record
                .dependencies
                .iter()
                .map(PersistedFrontendArtifactDependency::from_frontend)
                .collect(),
            diagnostics: record.diagnostics.iter().map(|source| source.0).collect(),
        }
    }

    fn into_frontend(self) -> Option<FrontendArtifactRecord> {
        Some(FrontendArtifactRecord {
            key: self.key.to_frontend()?,
            revision: ProjectRevision(self.revision),
            fingerprint: ArtifactFingerprint::new(self.fingerprint),
            dependencies: self
                .dependencies
                .iter()
                .map(PersistedFrontendArtifactDependency::to_frontend)
                .collect::<Option<Vec<_>>>()?,
            diagnostics: self.diagnostics.into_iter().map(SourceId).collect(),
        })
    }
}
