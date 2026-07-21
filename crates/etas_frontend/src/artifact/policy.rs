use super::{FrontendArtifactKey, FrontendArtifactKind, FrontendUnitKey};
use crate::BODY_UNIT_KIND;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistenceClass {
    MemoryOnly,
    DiskMetadataOnly,
    DiskPayload {
        priority: CachePriority,
        max_payload_bytes: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePriority {
    High,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiskCacheAccess {
    Disabled,
    ReadOnly,
    WriteOnly,
    #[default]
    ReadWrite,
}

impl DiskCacheAccess {
    pub const fn disabled() -> Self {
        Self::Disabled
    }

    pub const fn read_only() -> Self {
        Self::ReadOnly
    }

    pub const fn write_only() -> Self {
        Self::WriteOnly
    }

    pub const fn read_write() -> Self {
        Self::ReadWrite
    }

    pub fn can_read(self) -> bool {
        matches!(self, Self::ReadOnly | Self::ReadWrite)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::WriteOnly | Self::ReadWrite)
    }
}

pub const DEFAULT_BODY_FACT_MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_ITEM_SIGNATURE_MAX_PAYLOAD_BYTES: u64 = 512 * 1024;
pub const DEFAULT_MANIFEST_MAX_PAYLOAD_BYTES: u64 = 2 * 1024 * 1024;
pub const DEFAULT_REUSE_INDEX_MAX_PAYLOAD_BYTES: u64 = 4 * 1024 * 1024;

pub fn persistence_class(key: &FrontendArtifactKey) -> PersistenceClass {
    match (&key.kind, &key.unit) {
        (FrontendArtifactKind::ArtifactManifest, FrontendUnitKey::Project) => {
            PersistenceClass::DiskPayload {
                priority: CachePriority::High,
                max_payload_bytes: DEFAULT_MANIFEST_MAX_PAYLOAD_BYTES,
            }
        }
        (FrontendArtifactKind::BodyArtifactReuseIndex, FrontendUnitKey::Project) => {
            PersistenceClass::DiskPayload {
                priority: CachePriority::High,
                max_payload_bytes: DEFAULT_REUSE_INDEX_MAX_PAYLOAD_BYTES,
            }
        }
        (
            FrontendArtifactKind::SourceFingerprintSummary
            | FrontendArtifactKind::ModuleImportExportSummary
            | FrontendArtifactKind::ArtifactDependencySummary
            | FrontendArtifactKind::ArtifactReuseStats,
            FrontendUnitKey::Project,
        ) => PersistenceClass::DiskMetadataOnly,
        (FrontendArtifactKind::SignatureFacts, FrontendUnitKey::Item(_)) => {
            PersistenceClass::DiskPayload {
                priority: CachePriority::High,
                max_payload_bytes: DEFAULT_ITEM_SIGNATURE_MAX_PAYLOAD_BYTES,
            }
        }
        (
            FrontendArtifactKind::TypeFacts | FrontendArtifactKind::EffectFacts,
            FrontendUnitKey::Unit(unit),
        ) if unit.kind == BODY_UNIT_KIND => PersistenceClass::DiskPayload {
            priority: CachePriority::High,
            max_payload_bytes: DEFAULT_BODY_FACT_MAX_PAYLOAD_BYTES,
        },
        _ => PersistenceClass::MemoryOnly,
    }
}
