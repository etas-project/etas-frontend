use etas_cache::{ArtifactFingerprint, ArtifactKey, ProjectRevision};
use etas_core::SourceId;
use etas_hir::HirItemId;
use etas_utils::{UnitKey, UnitKindKey};

use crate::{AstItemKind, BODY_UNIT_KIND, ModuleId, ModulePartId};

use super::{
    FrontendArtifactDependency, FrontendArtifactKey, FrontendArtifactKind, FrontendUnitKey,
    fingerprint_text,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BodyArtifactReuseIndex {
    pub records: Vec<BodyArtifactReuseIndexRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BodyArtifactReuseIndexRecord {
    pub kind: BodyArtifactReuseKind,
    pub unit: u32,
    pub revision: u64,
    pub fingerprint: [u8; 32],
    pub identity: BodyArtifactIdentityRecord,
    pub dependencies: Vec<PersistedFrontendArtifactDependency>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BodyArtifactReuseKind {
    TypeFacts,
    EffectFacts,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BodyArtifactIdentityRecord {
    pub source: u32,
    pub item_index: usize,
    pub item_kind: AstItemKind,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedBodyArtifactReuseIndex {
    records: Vec<PersistedBodyArtifactReuseIndexRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedBodyArtifactReuseIndexRecord {
    kind: PersistedBodyArtifactReuseKind,
    unit: u32,
    revision: u64,
    fingerprint: [u8; 32],
    identity: PersistedBodyArtifactIdentityRecord,
    dependencies: Vec<PersistedFrontendArtifactDependency>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum PersistedBodyArtifactReuseKind {
    TypeFacts,
    EffectFacts,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedBodyArtifactIdentityRecord {
    source: u32,
    item_index: usize,
    item_kind: AstItemKind,
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedFrontendArtifactDependency {
    pub key: PersistedFrontendArtifactKey,
    pub fingerprint: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedFrontendArtifactKey {
    pub kind: FrontendArtifactKind,
    pub unit: PersistedFrontendUnitKey,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PersistedFrontendUnitKey {
    Project,
    Source(u32),
    Module(u32),
    ModulePart(u32),
    Item(u32),
    Unit {
        namespace: String,
        name: String,
        id: u64,
    },
}

impl BodyArtifactReuseIndex {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn fingerprint(&self) -> ArtifactFingerprint {
        let mut parts = vec!["body_artifact_reuse_index:v1".to_owned()];
        for record in &self.records {
            parts.extend(record.fingerprint_parts());
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        fingerprint_text(&refs)
    }

    pub fn dependencies(&self) -> Vec<ArtifactKey> {
        self.records
            .iter()
            .map(BodyArtifactReuseIndexRecord::artifact_key)
            .collect()
    }
}

impl PersistedBodyArtifactReuseIndex {
    pub(crate) fn from_frontend(index: &BodyArtifactReuseIndex) -> Self {
        Self {
            records: index
                .records
                .iter()
                .map(PersistedBodyArtifactReuseIndexRecord::from_frontend)
                .collect(),
        }
    }

    pub(crate) fn into_frontend(self) -> Option<BodyArtifactReuseIndex> {
        Some(BodyArtifactReuseIndex {
            records: self
                .records
                .into_iter()
                .map(PersistedBodyArtifactReuseIndexRecord::into_frontend)
                .collect::<Option<Vec<_>>>()?,
        })
    }
}

impl PersistedBodyArtifactReuseIndexRecord {
    fn from_frontend(record: &BodyArtifactReuseIndexRecord) -> Self {
        Self {
            kind: record.kind.into(),
            unit: record.unit,
            revision: record.revision,
            fingerprint: record.fingerprint,
            identity: PersistedBodyArtifactIdentityRecord::from_frontend(&record.identity),
            dependencies: record.dependencies.clone(),
        }
    }

    fn into_frontend(self) -> Option<BodyArtifactReuseIndexRecord> {
        if !self
            .dependencies
            .iter()
            .all(|dependency| dependency.to_frontend().is_some())
        {
            return None;
        }
        Some(BodyArtifactReuseIndexRecord {
            kind: self.kind.into(),
            unit: self.unit,
            revision: self.revision,
            fingerprint: self.fingerprint,
            identity: self.identity.into_frontend(),
            dependencies: self.dependencies,
        })
    }
}

impl From<BodyArtifactReuseKind> for PersistedBodyArtifactReuseKind {
    fn from(kind: BodyArtifactReuseKind) -> Self {
        match kind {
            BodyArtifactReuseKind::TypeFacts => Self::TypeFacts,
            BodyArtifactReuseKind::EffectFacts => Self::EffectFacts,
        }
    }
}

impl From<PersistedBodyArtifactReuseKind> for BodyArtifactReuseKind {
    fn from(kind: PersistedBodyArtifactReuseKind) -> Self {
        match kind {
            PersistedBodyArtifactReuseKind::TypeFacts => Self::TypeFacts,
            PersistedBodyArtifactReuseKind::EffectFacts => Self::EffectFacts,
        }
    }
}

impl PersistedBodyArtifactIdentityRecord {
    fn from_frontend(identity: &BodyArtifactIdentityRecord) -> Self {
        Self {
            source: identity.source,
            item_index: identity.item_index,
            item_kind: identity.item_kind,
            text: identity.text.clone(),
        }
    }

    fn into_frontend(self) -> BodyArtifactIdentityRecord {
        BodyArtifactIdentityRecord {
            source: self.source,
            item_index: self.item_index,
            item_kind: self.item_kind,
            text: self.text,
        }
    }
}

impl BodyArtifactReuseIndexRecord {
    pub fn artifact_key(&self) -> ArtifactKey {
        self.frontend_key().to_cache_key()
    }

    pub fn frontend_key(&self) -> FrontendArtifactKey {
        let kind = match self.kind {
            BodyArtifactReuseKind::TypeFacts => FrontendArtifactKind::TypeFacts,
            BodyArtifactReuseKind::EffectFacts => FrontendArtifactKind::EffectFacts,
        };
        FrontendArtifactKey::unit(kind, UnitKey::new(BODY_UNIT_KIND, u64::from(self.unit)))
    }

    pub fn project_revision(&self) -> ProjectRevision {
        ProjectRevision(self.revision)
    }

    pub fn artifact_fingerprint(&self) -> ArtifactFingerprint {
        ArtifactFingerprint::new(self.fingerprint)
    }

    pub fn frontend_dependencies(&self) -> Option<Vec<FrontendArtifactDependency>> {
        self.dependencies
            .iter()
            .map(PersistedFrontendArtifactDependency::to_frontend)
            .collect()
    }

    fn fingerprint_parts(&self) -> Vec<String> {
        let mut parts = vec![
            format!("kind:{:?}", self.kind),
            format!("unit:{}", self.unit),
            format!("revision:{}", self.revision),
            format!("fingerprint:{:?}", self.fingerprint),
            format!("identity_source:{}", self.identity.source),
            format!("identity_item_index:{}", self.identity.item_index),
            format!("identity_item_kind:{:?}", self.identity.item_kind),
            self.identity.text.clone(),
        ];
        for dependency in &self.dependencies {
            parts.extend(dependency.fingerprint_parts());
        }
        parts
    }
}

impl PersistedFrontendArtifactDependency {
    pub fn from_frontend(dependency: &FrontendArtifactDependency) -> Self {
        Self {
            key: PersistedFrontendArtifactKey::from_frontend(&dependency.key),
            fingerprint: dependency.fingerprint.map(ArtifactFingerprint::bytes),
        }
    }

    pub fn to_frontend(&self) -> Option<FrontendArtifactDependency> {
        Some(FrontendArtifactDependency {
            key: self.key.to_frontend()?,
            fingerprint: self.fingerprint.map(ArtifactFingerprint::new),
        })
    }

    fn fingerprint_parts(&self) -> Vec<String> {
        vec![
            format!("dependency_key:{}", self.key.fingerprint_part()),
            format!("dependency_kind:{:?}", self.key.kind),
            format!("dependency_fingerprint:{:?}", self.fingerprint),
        ]
    }
}

impl PersistedFrontendArtifactKey {
    pub fn from_frontend(key: &FrontendArtifactKey) -> Self {
        Self {
            kind: key.kind,
            unit: PersistedFrontendUnitKey::from_frontend(&key.unit),
        }
    }

    pub fn to_frontend(&self) -> Option<FrontendArtifactKey> {
        Some(FrontendArtifactKey {
            kind: self.kind,
            unit: self.unit.to_frontend()?,
        })
    }

    fn fingerprint_part(&self) -> String {
        match &self.unit {
            PersistedFrontendUnitKey::Project => "project".to_owned(),
            PersistedFrontendUnitKey::Source(source) => format!("source:{source}"),
            PersistedFrontendUnitKey::Module(module) => format!("module:{module}"),
            PersistedFrontendUnitKey::ModulePart(part) => format!("module_part:{part}"),
            PersistedFrontendUnitKey::Item(item) => format!("item:{item}"),
            PersistedFrontendUnitKey::Unit {
                namespace,
                name,
                id,
            } => format!("unit:{namespace}:{name}:{id}"),
        }
    }
}

impl PersistedFrontendUnitKey {
    fn from_frontend(unit: &FrontendUnitKey) -> Self {
        match unit {
            FrontendUnitKey::Project => Self::Project,
            FrontendUnitKey::Source(source) => Self::Source(source.0),
            FrontendUnitKey::Module(module) => Self::Module(module.0),
            FrontendUnitKey::ModulePart(part) => Self::ModulePart(part.0),
            FrontendUnitKey::Item(item) => Self::Item(item.0),
            FrontendUnitKey::Unit(unit) => Self::Unit {
                namespace: unit.kind.namespace.to_owned(),
                name: unit.kind.name.to_owned(),
                id: unit.id,
            },
        }
    }

    fn to_frontend(&self) -> Option<FrontendUnitKey> {
        match self {
            Self::Project => Some(FrontendUnitKey::Project),
            Self::Source(source) => Some(FrontendUnitKey::Source(SourceId(*source))),
            Self::Module(module) => Some(FrontendUnitKey::Module(ModuleId(*module))),
            Self::ModulePart(part) => Some(FrontendUnitKey::ModulePart(ModulePartId(*part))),
            Self::Item(item) => Some(FrontendUnitKey::Item(HirItemId(*item))),
            Self::Unit {
                namespace,
                name,
                id,
            } => known_unit_kind(namespace, name)
                .map(|kind| FrontendUnitKey::Unit(UnitKey::new(kind, *id))),
        }
    }
}

fn known_unit_kind(namespace: &str, name: &str) -> Option<UnitKindKey> {
    match (namespace, name) {
        ("frontend", "body") => Some(BODY_UNIT_KIND),
        _ => None,
    }
}
